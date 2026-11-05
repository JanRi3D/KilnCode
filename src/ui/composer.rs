//! Bottom composer: message editor, attachments, model/effort picker, send/stop and the
//! integrated approval card (answers the provider's real permission request).

use crate::app::App;
use crate::icons as ic;
use crate::model::{ProviderKind, RuleDecision, suggest_rule};
use crate::provider::{self, ApprovalKind, ApprovalRequest, Decision};
use crate::theme::{self, fs, mono_fs, p, radius};
use crate::widgets::{self as w, Btn};
use egui::{Align, Color32, Frame, Layout, Margin, Rect, Sense, Stroke, Ui, vec2};

/// Returns the composer's total height (used to pad the timeline).
pub fn thread_composer(app: &mut App, ui: &mut Ui, id: &str) -> f32 {
    let Some(t) = app.thread(id).cloned() else { return 0.0 };
    let pal = p();
    let approval = app.approvals.get(id).and_then(|a| a.first()).cloned();
    let start = ui.cursor().bottom();
    let fill = if pal.dark { theme::rgba(17, 17, 17, 0.92) } else { pal.raised };
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, theme::rgba(255, 255, 255, if pal.dark { 0.05 } else { 0.0 }).max_alpha_or(pal.border)))
        .corner_radius(radius(theme::R_2XL))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                if let Some(a) = &approval {
                    approval_card(app, ui, &t, a);
                }
                let draft_key = id.to_string();
                let mut text = app.ui.drafts.remove(&draft_key).unwrap_or_default();
                let hint = if approval.is_some() { "Send feedback to decline this request" } else { "Ask anything. @ files, / commands" };
                let te_id = ui.scope_id().with(("composer", id));
                let (send, opposite) = send_keys(app, ui, te_id);
                Frame::new().inner_margin(Margin { left: 16, right: 16, top: 14, bottom: 8 }).show(ui, |ui| {
                    attachments_row(app, ui, id);
                    let r = ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .id(te_id)
                            .hint_text(egui::RichText::new(hint).color(pal.faint))
                            .frame(Frame::NONE)
                            .font(theme::ui(fs(14.0)))
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .margin(vec2(0.0, 0.0)),
                    );
                    if std::mem::take(&mut app.ui.focus_composer) {
                        r.request_focus();
                    }
                });
                w::row_top(ui, |ui| {
                    ui.add_space(8.0);
                    ui.allocate_ui_with_layout(vec2(32.0, 36.0), Layout::left_to_right(Align::Center), |ui| {
                        if w::icon_button_sized(ui, ic::PAPERCLIP, "Attach files", false, 28.0, 16.0, pal.muted).clicked() {
                            app.pick_attachments(Some(0));
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center).with_main_wrap(true), |ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        ui.set_min_height(36.0);
                        ui.add_space(8.0);
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if let Some(a) = &approval {
                            approval_buttons(app, ui, &t, a, &mut text);
                        } else {
                            let running = t.state.is_active() && app.sessions.contains_key(id);
                            let has_text = !text.trim().is_empty();
                            if running && !has_text {
                                if stop_button(ui).clicked() {
                                    app.interrupt(id);
                                }
                            } else if send_button(ui, has_text).clicked() || (send && has_text) {
                                let msg = std::mem::take(&mut text);
                                app.send(id, msg, false);
                            } else if opposite && has_text {
                                let msg = std::mem::take(&mut text);
                                app.send(id, msg, true);
                            }
                            model_picker(app, ui, Some(id), t.provider);
                        }
                    });
                });
                app.ui.drafts.insert(draft_key, text);
            });
        });
    ui.add_space(8.0);
    // Footer: workspace + branch.
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 24.0), Layout::left_to_right(Align::Center), |ui| {
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        w::icon(ui, if t.is_worktree { ic::FOLDER_GIT } else { ic::FOLDER }, 12.0, pal.muted);
        w::text(
            ui,
            if t.is_worktree {
                "Worktree"
            } else if t.project_id.is_some() {
                "Current checkout"
            } else {
                "No project"
            },
            12.0,
            pal.muted,
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(8.0);
            w::text(ui, t.branch.clone().unwrap_or_default(), 12.0, pal.muted);
            if t.branch.is_some() {
                w::icon(ui, ic::GIT_BRANCH, 12.0, pal.muted);
            }
        });
    });
    ui.add_space(20.0);
    let _ = start;
    ui.min_rect().height()
}

trait MaxAlpha {
    fn max_alpha_or(self, other: Color32) -> Color32;
}
impl MaxAlpha for Color32 {
    fn max_alpha_or(self, other: Color32) -> Color32 {
        if self.a() == 0 { other } else { self }
    }
}

/// Handles Enter / Ctrl+Enter per Settings → Chat before the text edit sees the key.
pub fn send_keys(app: &App, ui: &mut Ui, te_id: egui::Id) -> (bool, bool) {
    if !ui.memory(|m| m.has_focus(te_id)) {
        return (false, false);
    }
    let ctrl_mode = app.settings.send_shortcut != "Enter";
    ui.input_mut(|i| {
        let opp = i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Enter);
        let send = if ctrl_mode {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)
        } else {
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
        };
        (send, opp)
    })
}

fn attachments_row(app: &mut App, ui: &mut Ui, id: &str) {
    let pal = p();
    let list = app.ui.attachments.get(id).cloned().unwrap_or_default();
    if list.is_empty() {
        return;
    }
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for (i, a) in list.iter().enumerate() {
            Frame::new()
                .fill(pal.ctrl_bg)
                .stroke(Stroke::new(1.0, pal.input_border))
                .corner_radius(radius(theme::R_LG))
                .inner_margin(Margin::symmetric(8, 3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        w::icon(ui, ic::PAPERCLIP, 12.0, pal.muted);
                        w::text(ui, a.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(), 12.0, pal.fg);
                        if w::icon_button_sized(ui, ic::X, "Remove", false, 16.0, 10.0, pal.muted).clicked() {
                            remove = Some(i);
                        }
                    });
                });
        }
    });
    if let Some(i) = remove
        && let Some(l) = app.ui.attachments.get_mut(id)
    {
        l.remove(i);
    }
    ui.add_space(6.0);
}

pub fn send_button(ui: &mut Ui, enabled: bool) -> egui::Response {
    let pal = p();
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let bg = if enabled {
        if resp.hovered() { pal.accent.gamma_multiply(1.1) } else { pal.accent }
    } else {
        theme::rgba(245, 245, 245, if pal.dark { 0.12 } else { 0.0 }).max_alpha_or(pal.track)
    };
    ui.painter().circle_filled(rect.center(), 16.0, bg);
    w::paint_icon(ui, rect, ic::ARROW_UP, 15.0, if enabled { Color32::WHITE } else { pal.muted });
    resp.on_hover_text("Send message").on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn stop_button(ui: &mut Ui) -> egui::Response {
    let pal = p();
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    ui.painter().circle_filled(rect.center(), 16.0, if resp.hovered() { pal.stop.gamma_multiply(1.1) } else { pal.stop });
    ui.painter().rect_filled(Rect::from_center_size(rect.center(), vec2(9.0, 9.0)), radius(2), Color32::WHITE);
    resp.on_hover_text("Interrupt (Esc)").on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn effort_label(app: &App, e: &str) -> String {
    if e.is_empty() {
        return String::new();
    }
    if app.settings.effort_display == "Bars" {
        let n = match e {
            "minimal" | "none" => 1,
            "low" => 2,
            "medium" => 3,
            "high" => 4,
            _ => 5,
        };
        return "▮".repeat(n) + &"▯".repeat(5 - n);
    }
    let mut c = e.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Provider/model/effort picker. With `thread`, edits that thread; otherwise the new-thread draft.
pub fn model_picker(app: &mut App, ui: &mut Ui, thread: Option<&str>, provider_kind: ProviderKind) {
    let pal = p();
    let (model, effort, kind) = match thread.and_then(|t| app.thread(t)) {
        Some(t) => (t.model.clone(), t.effort.clone().unwrap_or_default(), t.provider),
        None => {
            let (dm, de) = app.default_model(app.draft.provider);
            let m = if app.draft.model.is_empty() { dm } else { app.draft.model.clone() };
            let e = if app.draft.effort.is_empty() { de } else { app.draft.effort.clone() };
            (m, e, app.draft.provider)
        }
    };
    let _ = provider_kind;
    let label = app.model_label(kind, &model);
    let eff = effort_label(app, &effort);
    let font = theme::ui_medium(fs(14.0));
    let lw = w::galley_width(ui, &label, &font).min(220.0);
    let ew = if eff.is_empty() { 0.0 } else { w::galley_width(ui, &eff, &font) + 6.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(16.0 + 22.0 + lw + ew + 8.0, 28.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, radius(theme::R_LG), pal.hover);
    }
    w::paint_icon(ui, Rect::from_min_size(rect.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), ic::BOT, 16.0, pal.muted);
    w::paint_truncated(ui, rect.min + vec2(30.0, 14.0), egui::Align2::LEFT_CENTER, &label, font.clone(), pal.fg.gamma_multiply(0.85), lw);
    if !eff.is_empty() {
        ui.painter().text(rect.min + vec2(30.0 + lw + 6.0, 14.0), egui::Align2::LEFT_CENTER, &eff, font, pal.muted);
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    egui::Popup::menu(&resp).width(300.0).frame(w::popover(10).inner_margin(4)).show(|ui| {
        ui.set_min_width(290.0);
        let kinds: Vec<ProviderKind> = if thread.is_some() { vec![kind] } else { ProviderKind::ALL.to_vec() };
        for k in kinds {
            let st = app.providers.get(&k).cloned();
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                w::text_medium(ui, k.label(), 12.0, pal.muted);
                if let Some(s) = &st {
                    let ok = s.ready();
                    w::text(
                        ui,
                        if ok { "· ready".to_string() } else { format!("· {}", s.summary().to_lowercase()) },
                        12.0,
                        if ok { pal.success } else { pal.warning },
                    );
                }
            });
            if !app.provider_enabled(k) {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    w::text(ui, "Turned off in Settings → Providers", 12.0, pal.faint);
                });
                continue;
            }
            let models = app.models_for(k);
            if models.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    w::text(ui, "Models load after the first health check.", 12.0, pal.faint);
                });
            }
            for m in &models {
                let on = k == kind && m.id == model;
                let disp = if m.display.is_empty() { m.id.clone() } else { m.display.clone() };
                if w::menu_item(ui, on.then_some(ic::CHECK), &disp, m.is_default.then_some("default"), true).clicked() {
                    match thread {
                        Some(tid) => {
                            if let Some(t) = app.thread_mut(tid) {
                                t.model = m.id.clone();
                                if !m.efforts.is_empty() && !m.efforts.contains(&effort) {
                                    t.effort = m.default_effort.clone();
                                }
                            }
                            restart_if_idle(app, tid);
                        }
                        None => {
                            app.draft.provider = k;
                            app.draft.model = m.id.clone();
                            if !m.efforts.contains(&app.draft.effort) {
                                app.draft.effort = m.default_effort.clone().unwrap_or_default();
                            }
                        }
                    }
                    ui.close();
                }
            }
            w::menu_separator(ui);
        }
        let efforts =
            app.models_for(kind).into_iter().find(|m| m.id == model).map(|m| m.efforts).filter(|e| !e.is_empty()).unwrap_or_else(|| {
                if kind == ProviderKind::Claude {
                    vec!["low".into(), "medium".into(), "high".into(), "xhigh".into(), "max".into()]
                } else {
                    vec![]
                }
            });
        if !efforts.is_empty() && provider::capabilities(kind).effort {
            w::menu_heading(ui, "Reasoning effort");
            for e in efforts {
                if w::menu_item(ui, (e == effort).then_some(ic::CHECK), &effort_label(app, &e), None, true).clicked() {
                    match thread {
                        Some(tid) => {
                            if let Some(t) = app.thread_mut(tid) {
                                t.effort = Some(e.clone());
                            }
                            restart_if_idle(app, tid);
                        }
                        None => app.draft.effort = e,
                    }
                    ui.close();
                }
            }
        }
        if thread.is_some() {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("Changes apply from the next message. The provider session is resumed with the new setting.")
                            .font(theme::ui(fs(11.0)))
                            .color(pal.faint),
                    )
                    .wrap(),
                );
            });
        }
    });
}

/// Model/effort changes take effect on the next spawn (with `--resume` / `thread/resume`).
fn restart_if_idle(app: &mut App, id: &str) {
    let idle = app.thread(id).is_some_and(|t| !t.state.is_active());
    if idle {
        app.sessions.remove(id);
    }
    if let Some(t) = app.thread(id) {
        app.store.save_thread(t);
    }
}

pub fn risk_of(a: &ApprovalRequest, cwd: &str) -> (&'static str, Color32, &'static str) {
    let pal = p();
    let cmd = a.command.clone().unwrap_or_default().to_lowercase();
    let outside = a.paths.iter().any(|x| !crate::app::inside(cwd, x));
    let dangerous = [
        "rm -rf",
        "rm -r ",
        "remove-item",
        "del /s",
        "rmdir /s",
        "git push --force",
        "git push -f",
        "reset --hard",
        "git clean",
        "sudo ",
        "chmod -r",
        "format ",
        "mkfs",
        "| sh",
        "| bash",
        "curl ",
        "wget ",
        "invoke-webrequest",
    ]
    .iter()
    .any(|d| cmd.contains(d));
    if outside || a.network || dangerous || a.kind == ApprovalKind::Permissions {
        ("High risk", pal.danger, ic::SHIELD_ALERT)
    } else if a.read_only
        || [
            "cargo test",
            "cargo check",
            "cargo build",
            "cargo clippy",
            "npm test",
            "pnpm test",
            "yarn test",
            "go test",
            "pytest",
            "dotnet test",
            "git status",
            "git diff",
            "git log",
        ]
        .iter()
        .any(|s| cmd.starts_with(s))
    {
        ("Low risk", pal.success, ic::SHIELD)
    } else {
        ("Review", pal.warning, ic::SHIELD_QUESTION)
    }
}

fn approval_card(app: &mut App, ui: &mut Ui, t: &crate::model::Thread, a: &ApprovalRequest) {
    let pal = p();
    let cwd = app.cwd_of(t).map(|c| c.to_string_lossy().to_string()).unwrap_or_default();
    let (risk, color, glyph) = risk_of(a, &cwd);
    let more = app.approvals.get(&t.id).map(|l| l.len()).unwrap_or(1).saturating_sub(1);
    Frame::new()
        .fill(theme::tint(color, 0.06))
        .stroke(Stroke::new(1.0, theme::tint(color, 0.3)))
        .corner_radius(radius(theme::R_XL))
        .inner_margin(Margin::symmetric(12, 10))
        .outer_margin(Margin { left: 12, right: 12, top: 12, bottom: 0 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal_wrapped(|ui| {
                w::icon(ui, glyph, 16.0, color);
                w::text_medium(ui, &a.title, 14.0, pal.fg);
                w::badge(ui, risk, color, theme::tint(color, 0.16))
                    .on_hover_text("Kiln's assessment from the command and paths. The provider enforces your decision.");
                w::text(ui, format!("· {}", t.permissions), 14.0, pal.muted);
                if more > 0 {
                    w::text(ui, format!("· {more} more waiting"), 12.0, pal.muted);
                }
            });
            if let Some(cmd) = &a.command {
                Frame::new()
                    .fill(theme::rgba(0, 0, 0, if pal.dark { 0.4 } else { 0.04 }))
                    .stroke(Stroke::new(1.0, pal.border))
                    .corner_radius(radius(theme::R_LG))
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(egui::RichText::new(cmd).font(theme::mono(mono_fs(12.5))).color(pal.fg)).wrap());
                    });
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let fact = |ui: &mut Ui, g: &str, label: String, c: Color32| {
                    Frame::new()
                        .fill(theme::rgba(255, 255, 255, 0.03))
                        .stroke(Stroke::new(1.0, pal.border))
                        .corner_radius(radius(255))
                        .inner_margin(Margin::symmetric(8, 3))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 5.0;
                                w::icon(ui, g, 12.0, c);
                                w::text(ui, label, 12.0, c);
                            });
                        });
                };
                if let Some(tool) = &a.tool_name {
                    fact(ui, ic::WRENCH, tool.clone(), pal.ctx_fg);
                }
                if !a.paths.is_empty() {
                    let outside = a.paths.iter().any(|x| !crate::app::inside(&cwd, x));
                    if outside {
                        fact(ui, ic::SHIELD_ALERT, "Outside the worktree".into(), pal.danger);
                    } else {
                        fact(ui, ic::FILE_PEN, format!("Inside this {}", if t.is_worktree { "worktree" } else { "checkout" }), pal.ctx_fg);
                    }
                }
                if a.read_only {
                    fact(ui, ic::EYE, "Reads only".into(), pal.success);
                }
                if a.network {
                    fact(ui, ic::GLOBE, "Network access".into(), pal.warning);
                }
                if let Some(c) = &a.cwd {
                    fact(
                        ui,
                        ic::FOLDER,
                        provider::short(
                            &crate::provider::claude::rel_path(&crate::log::home_dir().to_string_lossy(), c).replace('\\', "/"),
                            48,
                        ),
                        pal.muted,
                    );
                }
            });
            if let Some(pv) = &a.preview {
                egui::ScrollArea::vertical().id_salt(("preview", &a.request_id)).max_height(140.0).show(ui, |ui| {
                    Frame::new()
                        .fill(theme::rgba(0, 0, 0, if pal.dark { 0.4 } else { 0.04 }))
                        .stroke(Stroke::new(1.0, pal.border))
                        .corner_radius(radius(theme::R_LG))
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let mut job = egui::text::LayoutJob::default();
                            for l in pv.lines() {
                                let c = if l.starts_with('+') {
                                    pal.add
                                } else if l.starts_with('-') {
                                    pal.del
                                } else {
                                    pal.ctx_fg
                                };
                                job.append(&format!("{l}\n"), 0.0, egui::TextFormat::simple(theme::mono(mono_fs(12.0)), c));
                            }
                            ui.add(egui::Label::new(job).wrap());
                        });
                });
            }
            if let Some(r) = a.reason.as_ref().filter(|r| !r.trim().is_empty()) {
                ui.add(
                    egui::Label::new(egui::RichText::new(format!("Agent: “{}”", r.trim())).font(theme::ui(fs(12.0))).color(pal.muted))
                        .wrap(),
                );
            }
        });
}

fn approval_buttons(app: &mut App, ui: &mut Ui, t: &crate::model::Thread, a: &ApprovalRequest, text: &mut String) {
    let id = t.id.clone();
    if Btn::primary("Allow once").show(ui).on_hover_text("Ctrl+Enter").clicked() {
        app.decide(&id, &a.request_id, Decision::Allow, None);
    }
    let rule = (a.kind == ApprovalKind::Command).then(|| a.command.as_deref().and_then(suggest_rule)).flatten();
    if let Some(r) = &rule
        && Btn::new(&format!("Always allow “{}”", provider::short(r, 28)))
            .icon(ic::SHIELD_CHECK)
            .tooltip("Saves a Kiln rule for this project. Manage rules in Settings → Approvals.")
            .show(ui)
            .clicked()
    {
        app.add_rule(t.project_id.clone(), r.clone(), RuleDecision::Allow);
        let label = format!("Approved {} · saved rule “{r}”", provider::short(a.command.as_deref().unwrap_or(""), 50));
        app.decide(&id, &a.request_id, Decision::Allow, Some(label));
        let tt = app.time();
        app.toasts.push(crate::app::Toast {
            text: format!("Saved a rule: always allow “{r}” in {}.", app.project_name(t)),
            ok: true,
            until: tt + 4.5,
            link_rules: true,
        });
    }
    if let Some(s) = &a.session_option
        && Btn::new(&provider::short(s, 36)).tooltip("The provider remembers this for the current session only.").show(ui).clicked()
    {
        app.decide(&id, &a.request_id, Decision::AllowForSession, None);
    }
    let feedback = text.trim().to_string();
    if Btn::new("Decline")
        .show(ui)
        .on_hover_text(if feedback.is_empty() {
            "Type feedback in the box to tell the agent why"
        } else {
            "Declines and sends your feedback"
        })
        .clicked()
    {
        app.decide(&id, &a.request_id, Decision::Deny { message: (!feedback.is_empty()).then_some(feedback) }, None);
        text.clear();
    }
    // Provider-native "decline and stop the turn" (Codex `cancel`, Claude deny with interrupt).
    if w::icon_button_sized(ui, ic::STOP_SQUARE, "Decline and stop the turn", false, 24.0, 12.0, p().muted).clicked() {
        app.decide(
            &id,
            &a.request_id,
            Decision::Abort,
            Some(format!("Declined and stopped: {}", provider::short(a.command.as_deref().unwrap_or(&a.title), 70))),
        );
    }
}
