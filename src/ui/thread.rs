//! Workspace — thread: header, conversation, thread details, terminal drawer, right panel.

use super::{composer, diff, markdown};
use crate::app::{App, Confirm, RightTab, View};
use crate::icons as ic;
use crate::model::*;
use crate::provider;
use crate::theme::{self, fs, mono_fs, p, radius};
use crate::widgets::{self as w, Btn};
use egui::{Align, Align2, Frame, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};

pub fn show(app: &mut App, ui: &mut Ui) {
    let Some(id) = app.current.clone().filter(|id| app.thread(id).is_some()) else {
        app.view = View::NewThread;
        return;
    };
    let full = ui.available_rect_before_wrap();
    let narrow = full.width() < 760.0;
    header(app, ui, &id, narrow);
    if app.ui.terminal_open {
        egui::Panel::bottom("terminal_drawer")
            .resizable(true)
            .default_size(220.0)
            .size_range(120.0..=(full.height() * 0.7).max(140.0))
            .frame(Frame::new().fill(p().bg))
            .show(ui, |ui| terminal_drawer(app, ui, &id));
    }
    let area = ui.available_rect_before_wrap();
    let details = app.ui.details_open && !narrow && area.width() > 900.0;
    let lane_pad = if narrow { 12.0 } else { 48.0 };
    let right_pad = if details { 308.0 } else { lane_pad };
    let composer_h = if app.ui.composer_h.is_finite() { app.ui.composer_h.clamp(120.0, area.height() * 0.8) } else { 150.0 };
    let mut tl = ui.new_child(egui::UiBuilder::new().max_rect(area));
    timeline(app, &mut tl, &id, lane_pad, right_pad, composer_h + 12.0);

    // Composer floats over the bottom of the conversation.
    let max_w = app.settings.chat_max_width().min(area.width() - lane_pad - right_pad);
    let cx = area.left() + lane_pad + (area.width() - lane_pad - right_pad - max_w) / 2.0;
    let crect = Rect::from_min_max(pos2(cx, area.bottom() - composer_h), pos2(cx + max_w, area.bottom()));
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(crect).layout(Layout::top_down(Align::Min)));
    let used = composer::thread_composer(app, &mut cui, &id);
    if (used - app.ui.composer_h).abs() > 0.5 {
        ui.ctx().request_repaint();
    }
    app.ui.composer_h = used;

    if details {
        let r = Rect::from_min_size(pos2(area.right() - 12.0 - 280.0, area.top()), vec2(280.0, 10.0));
        let mut dui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(r.min, vec2(280.0, area.height()))));
        thread_details(app, &mut dui, &id);
    }
    ui.allocate_rect(area, Sense::hover());
}

fn status_pill(app: &App, t: &Thread) -> (String, &'static str, egui::Color32, bool) {
    let pal = p();
    let pending = app.approvals.get(&t.id).is_some_and(|a| !a.is_empty());
    let live = t.turn_started_at.map(|s| now() - s).unwrap_or(0);
    match t.state {
        _ if pending => ("Needs approval".into(), ic::SHIELD_QUESTION, pal.warning, false),
        RunState::Working => (format!("Working · {}", w::duration(live)), ic::CIRCLE_DASHED, pal.info, true),
        RunState::Queued => ("Queued".into(), ic::CIRCLE_DASHED, pal.muted, false),
        RunState::NeedsApproval => ("Needs approval".into(), ic::SHIELD_QUESTION, pal.warning, false),
        RunState::ReadyToReview | RunState::Completed => {
            (format!("Done in {}", w::duration(t.worked_secs)), ic::CIRCLE_CHECK, pal.success, false)
        }
        RunState::Failed => ("Failed".into(), ic::CIRCLE_X, pal.danger, false),
        RunState::Cancelled => ("Stopped".into(), ic::CIRCLE_X, pal.muted, false),
        RunState::Idle => ("Idle".into(), ic::CHECK, pal.muted, false),
    }
}

fn header(app: &mut App, ui: &mut Ui, id: &str, narrow: bool) {
    let Some(t) = app.thread(id).cloned() else { return };
    let pal = p();
    let right_pad = if app.ui.right_open { 20.0 } else { 152.0 };
    let pname = app.project_name(&t);
    super::header(ui, right_pad, |ui| {
        if !app.ui.sidebar_open && w::icon_button(ui, ic::PANEL_LEFT, "Show sidebar", false).clicked() {
            app.ui.sidebar_open = true;
        }
        if super::crumb_project(ui, &pname).clicked() {
            app.draft.project = t.project_id.clone();
            app.view = View::NewThread;
        }
        let title = w::text_medium(ui, &t.title, 14.0, pal.fg);
        let menu = w::icon_button_sized(ui, ic::CHEVRON_DOWN, "Thread actions", false, 20.0, 14.0, pal.muted);
        let _ = title;
        egui::Popup::menu(&menu).width(220.0).frame(w::popover(10).inner_margin(4)).show(|ui| {
            ui.set_min_width(210.0);
            if w::menu_item(ui, Some(ic::SQUARE_PEN), "Rename", None, true).clicked() {
                app.ui.rename = Some((t.id.clone(), t.title.clone()));
                ui.close();
            }
            if w::menu_item(ui, Some(ic::PIN), if t.pinned { "Unpin" } else { "Pin to top" }, None, true).clicked() {
                app.toggle_pin(&t.id);
                ui.close();
            }
            if w::menu_item(ui, Some(ic::BADGE_CHECK), "Review & ship", None, true).clicked() {
                app.view = View::Review;
                ui.close();
            }
            if t.is_worktree && t.cwd.is_some() && w::menu_item(ui, Some(ic::FOLDER_GIT), "Remove worktree…", None, true).clicked() {
                app.confirm = Some(Confirm::RemoveWorktree { thread: t.id.clone() });
                ui.close();
            }
            w::menu_separator(ui);
            if w::menu_item(ui, Some(ic::ARCHIVE), "Archive", None, true).clicked() {
                app.set_archived(&t.id, true);
                ui.close();
            }
            if w::menu_item(ui, Some(ic::TRASH), "Delete…", None, true).clicked() {
                app.confirm = Some(Confirm::DeleteThread { thread: t.id.clone() });
                ui.close();
            }
        });
        if narrow {
            return;
        }
        let (label, glyph, color, spin) = status_pill(app, &t);
        w::pill(ui, Some(glyph), &label, color, 22.0, spin);
        if !t.plan.is_empty() {
            let done = t.plan.iter().filter(|s| s.status == StepStatus::Completed).count();
            let lbl = format!("Plan {done}/{}", t.plan.len());
            let r = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                w::icon(ui, ic::LIST_CHECKS, 12.0, pal.muted);
                w::text(ui, &lbl, 12.0, pal.muted);
                w::progress(ui, 40.0, done as f32 / t.plan.len() as f32, color);
            });
            let resp = r.response.interact(Sense::click()).on_hover_text("Show the plan").on_hover_cursor(egui::CursorIcon::PointingHand);
            egui::Popup::menu(&resp).width(320.0).frame(w::popover(12).inner_margin(egui::Margin::symmetric(12, 10))).show(|ui| {
                ui.set_width(296.0);
                w::text_medium(ui, format!("Plan · {lbl}"), 11.0, pal.muted);
                ui.add_space(4.0);
                for s in &t.plan {
                    ui.horizontal(|ui| {
                        let (g, c) = match s.status {
                            StepStatus::Completed => (ic::CIRCLE_CHECK, pal.success),
                            StepStatus::InProgress => (ic::CIRCLE_DASHED, pal.info),
                            StepStatus::Pending => (ic::CIRCLE, pal.line_no),
                        };
                        w::icon(ui, g, 14.0, c);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&s.step).font(theme::ui(fs(13.0))).color(if s.status == StepStatus::Pending {
                                    pal.muted
                                } else {
                                    pal.fg
                                }),
                            )
                            .wrap(),
                        );
                    });
                }
            });
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if let Some(c) = t.usage.est_cost_usd {
                w::text(ui, format!("${c:.2} est."), 12.0, pal.muted)
                    .on_hover_text("API-equivalent cost reported by Claude Code. This is an estimate, not a charge to your subscription.");
            }
            if let (Some(used), Some(win)) = (t.usage.context_tokens, t.usage.context_window) {
                let pct = (used as f64 / win.max(1) as f64 * 100.0).round();
                w::text(ui, format!("{pct:.0}% context"), 12.0, pal.muted)
                    .on_hover_text(format!("{used} of {win} tokens in the context window, as reported by {}.", t.provider.label()));
            }
        });
    });
}

// ------------------------------------------------------------------------------- timeline

struct Turn {
    start: usize,
    end: usize,
    done: Option<i64>,
}

fn turns(items: &[Item]) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    let mut start = 0;
    for (i, it) in items.iter().enumerate() {
        if matches!(it.kind, ItemKind::User { .. }) && i > start {
            out.push(Turn { start, end: i, done: None });
            start = i;
        }
    }
    if start < items.len() {
        out.push(Turn { start, end: items.len(), done: None });
    }
    for t in &mut out {
        t.done = items[t.start..t.end].iter().find_map(|it| if let ItemKind::TurnEnd { secs, .. } = it.kind { Some(secs) } else { None });
    }
    out
}

fn timeline(app: &mut App, ui: &mut Ui, id: &str, left: f32, right: f32, bottom: f32) {
    let items = app.items.get(id).cloned().unwrap_or_default();
    let Some(t) = app.thread(id).cloned() else { return };
    let ts = turns(&items);
    let max_w = app.settings.chat_max_width();
    let stick = std::mem::take(&mut app.ui.scroll_to_bottom);
    let mut sa = egui::ScrollArea::vertical().id_salt(("timeline", id)).auto_shrink([false, false]).stick_to_bottom(true);
    if stick {
        sa = sa.vertical_scroll_offset(1.0e7);
    }
    sa.show_viewport(ui, |ui, vp| {
        let avail = ui.available_width();
        let lane = (avail - left - right).min(max_w).max(200.0);
        let x_pad = left + (avail - left - right - lane).max(0.0) / 2.0;
        ui.add_space(12.0);
        if ts.is_empty() {
            ui.add_space(40.0);
            ui.horizontal(|ui| {
                ui.add_space(x_pad);
                w::text(ui, "No messages yet.", 13.0, p().faint);
            });
        }
        let n = ts.len();
        for (k, turn) in ts.iter().enumerate() {
            let last = k + 1 == n;
            let key = ui.scope_id().with(("turn_h", id, turn.start, lane as i32));
            let cached: Option<f32> = ui.data(|d| d.get_temp(key));
            let y0 = ui.cursor().top() - ui.min_rect().top();
            if let Some(h) = cached.filter(|_| !last)
                && (y0 + h < vp.min.y - 400.0 || y0 > vp.max.y + 400.0)
            {
                ui.allocate_space(vec2(avail, h));
                continue;
            }
            let top = ui.cursor().top();
            ui.horizontal(|ui| {
                ui.add_space(x_pad);
                ui.vertical(|ui| {
                    ui.set_width(lane);
                    render_turn(app, ui, &t, &items[turn.start..turn.end], turn.done, last);
                });
            });
            let h = ui.cursor().top() - top;
            ui.data_mut(|d| d.insert_temp(key, h));
        }
        if let Some(th) = app.thread(id)
            && !th.queued.is_empty()
        {
            ui.horizontal(|ui| {
                ui.add_space(x_pad);
                ui.vertical(|ui| {
                    ui.set_width(lane);
                    for q in &th.queued {
                        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                            w::badge(ui, "Queued", p().muted, p().kbd_bg);
                            w::text(ui, provider::short(q, 70), 12.0, p().muted);
                        });
                    }
                });
            });
        }
        ui.add_space(bottom);
    });
}

fn render_turn(app: &mut App, ui: &mut Ui, t: &Thread, items: &[Item], done: Option<i64>, last: bool) {
    let pal = p();
    ui.spacing_mut().item_spacing.y = 0.0;
    let final_asst = items.iter().rposition(|i| matches!(&i.kind, ItemKind::Assistant { text } if !text.trim().is_empty()));
    let mut fold: Vec<&Item> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        match &it.kind {
            ItemKind::User { text, attachments } => user_bubble(ui, text, attachments),
            ItemKind::Assistant { text } if done.is_none() || Some(i) == final_asst => {
                if done.is_some() && !fold.is_empty() {
                    worked_fold(app, ui, &fold, done.unwrap_or(0));
                    fold.clear();
                }
                if !text.trim().is_empty() {
                    Frame::new().inner_margin(Margin { left: 4, right: 4, top: 2, bottom: 8 }).show(ui, |ui| {
                        markdown::show(ui, text, fs(14.0), pal.fg_soft, app.settings.word_wrap);
                        ui.horizontal(|ui| {
                            if w::icon_button_sized(ui, ic::COPY, "Copy message", false, 22.0, 13.0, pal.faint).clicked() {
                                ui.ctx().copy_text(text.clone());
                            }
                        });
                    });
                }
            }
            ItemKind::Error { text } => {
                if !fold.is_empty() && done.is_some() {
                    fold.push(it);
                    continue;
                }
                notice(ui, ic::ALERT, pal.danger, text);
            }
            ItemKind::Notice { text } => notice(ui, ic::INFO, pal.info, text),
            ItemKind::TurnEnd { .. } => {}
            _ if done.is_some() => fold.push(it),
            ItemKind::Tool { .. } | ItemKind::Decision { .. } | ItemKind::Reasoning { .. } | ItemKind::Assistant { .. } => {
                activity_row(app, ui, it, false)
            }
        }
    }
    if done.is_some() && !fold.is_empty() {
        worked_fold(app, ui, &fold, done.unwrap_or(0));
    }
    if last {
        let g = app.git.get(&t.id).map(|g| g.totals()).unwrap_or((0, 0, 0));
        if done.is_some() && g.0 > 0 {
            files_card(app, ui, t);
        }
        if done.is_none() && t.state.is_active() {
            let pending = app.approvals.get(&t.id).is_some_and(|a| !a.is_empty());
            let label = if pending {
                "Waiting for your approval".to_string()
            } else if t.state == RunState::Queued {
                t.last_activity.clone().unwrap_or_else(|| "Queued".into())
            } else {
                format!("Working for {}", w::duration(t.turn_started_at.map(|s| now() - s).unwrap_or(0)))
            };
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                if !pending {
                    w::spinner(ui, 12.0, pal.muted);
                }
                w::text(ui, label, 14.0, pal.muted);
                if let Some(a) = t.last_activity.as_ref().filter(|_| !pending && t.state == RunState::Working) {
                    w::text(ui, format!("· {}", provider::short(a, 60)), 13.0, pal.faint);
                }
            });
            ui.add_space(8.0);
            w::hairline(ui, pal.border_soft);
            ui.add_space(8.0);
        }
    }
}

fn user_bubble(ui: &mut Ui, text: &str, attachments: &[String]) {
    let pal = p();
    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
        let max = ui.available_width() * 0.8;
        Frame::new().fill(pal.bubble).corner_radius(radius(theme::R_2XL)).inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_max_width(max);
            ui.vertical(|ui| {
                if text.contains("```") {
                    // Multi-line code (e.g. review comments sent to the agent) renders as Markdown.
                    markdown::show(ui, text, fs(14.0), pal.fg, true);
                } else {
                    ui.add(egui::Label::new(markdown::inline_job(text, fs(14.0), pal.fg)).wrap());
                }
                for a in attachments {
                    ui.horizontal(|ui| {
                        w::icon(ui, ic::PAPERCLIP, 12.0, pal.muted);
                        w::text(
                            ui,
                            std::path::Path::new(a).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| a.clone()),
                            12.0,
                            pal.muted,
                        );
                    });
                }
            });
        });
    });
    ui.add_space(16.0);
}

fn notice(ui: &mut Ui, glyph: &str, color: egui::Color32, text: &str) {
    Frame::new()
        .fill(theme::tint(color, 0.06))
        .stroke(Stroke::new(1.0, theme::tint(color, 0.25)))
        .corner_radius(radius(theme::R_LG))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            w::row_top(ui, |ui| {
                w::icon(ui, glyph, 14.0, color);
                ui.add(egui::Label::new(egui::RichText::new(text).font(theme::ui(fs(13.0))).color(p().fg)).wrap());
            });
        });
    ui.add_space(10.0);
}

fn worked_fold(app: &mut App, ui: &mut Ui, items: &[&Item], secs: i64) {
    let pal = p();
    let key = format!("fold:{}", items[0].id);
    let open = app.ui.folds.contains(&key);
    Frame::new().inner_margin(Margin { left: 0, right: 2, top: 4, bottom: 8 }).show(ui, |ui| {
        let label = format!("Worked for {}", w::duration(secs));
        let r = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_space(4.0);
            w::text(ui, &label, 14.0, pal.muted);
            w::icon(ui, if open { ic::CHEVRON_DOWN } else { ic::CHEVRON_RIGHT }, 14.0, pal.muted);
        });
        if r.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            if open {
                app.ui.folds.remove(&key);
            } else {
                app.ui.folds.insert(key.clone());
            }
        }
        if open {
            ui.add_space(4.0);
            for it in items {
                activity_row(app, ui, it, true);
            }
        }
    });
    w::hairline(ui, pal.border_soft);
    ui.add_space(8.0);
}

fn tool_icon(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => ic::EYE,
        ToolKind::Search | ToolKind::List => ic::SEARCH,
        ToolKind::Command => ic::TERMINAL,
        ToolKind::Edit => ic::SQUARE_PEN,
        ToolKind::Web => ic::GLOBE,
        ToolKind::Mcp => ic::WRENCH,
        ToolKind::Task => ic::BOT,
        ToolKind::Other => ic::WRENCH,
    }
}

fn activity_row(app: &mut App, ui: &mut Ui, it: &Item, in_fold: bool) {
    let pal = p();
    let key = format!("row:{}", it.id);
    let open = app.ui.folds.contains(&key);
    let (glyph, label, extra, extra_color, expandable, spin) = match &it.kind {
        ItemKind::Tool { kind, label, status, output, detail, .. } => {
            let (ex, c) = match status {
                ToolStatus::Running => (String::new(), pal.muted),
                ToolStatus::Ok => (String::new(), pal.muted),
                ToolStatus::Failed => ("failed".into(), pal.danger),
                ToolStatus::Declined => ("declined".into(), pal.muted),
            };
            (
                tool_icon(*kind),
                label.clone(),
                ex,
                c,
                output.as_ref().is_some_and(|o| !o.trim().is_empty()) || detail.is_some(),
                *status == ToolStatus::Running,
            )
        }
        ItemKind::Decision { label, allowed } => {
            (if *allowed { ic::SHIELD_CHECK } else { ic::X }, label.clone(), String::new(), pal.muted, false, false)
        }
        ItemKind::Reasoning { text } => (
            ic::BRAIN,
            if in_fold { "Thought".into() } else { "Thinking".into() },
            String::new(),
            pal.muted,
            !text.trim().is_empty(),
            false,
        ),
        ItemKind::Assistant { text } => {
            (ic::BOT, provider::short(text, 120), String::new(), pal.muted, text.len() > 120 || text.contains('\n'), false)
        }
        ItemKind::Error { text } => (ic::ALERT, provider::short(text, 120), String::new(), pal.danger, true, false),
        _ => return,
    };
    let r = ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        ui.spacing_mut().item_spacing.x = 6.0;
        let (ir, _) = ui.allocate_exact_size(vec2(24.0, 20.0), Sense::hover());
        let ic_rect = Rect::from_center_size(ir.center(), vec2(16.0, 16.0));
        if spin {
            w::paint_spinner(ui, ic_rect, pal.muted)
        } else {
            w::paint_icon(ui, ic_rect, glyph, 16.0, pal.muted)
        }
        let max = ui.available_width() - 60.0;
        let lr = w::paint_truncated(
            ui,
            pos2(ui.cursor().left(), ir.center().y),
            Align2::LEFT_CENTER,
            &label,
            theme::ui(fs(14.0)),
            pal.muted,
            max,
        );
        ui.allocate_exact_size(vec2(lr.width(), 20.0), Sense::hover());
        if !extra.is_empty() {
            w::text(ui, &extra, 12.0, extra_color);
        }
        if expandable {
            w::icon(ui, if open { ic::CHEVRON_DOWN } else { ic::CHEVRON_RIGHT }, 12.0, pal.faint);
        }
    });
    if expandable && r.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        if open {
            app.ui.folds.remove(&key);
        } else {
            app.ui.folds.insert(key.clone());
        }
    }
    if open {
        let body = match &it.kind {
            ItemKind::Tool { detail, output, .. } => {
                [detail.clone(), output.clone()].into_iter().flatten().collect::<Vec<_>>().join("\n\n")
            }
            ItemKind::Reasoning { text } | ItemKind::Assistant { text } | ItemKind::Error { text } => text.clone(),
            _ => String::new(),
        };
        Frame::new()
            .fill(pal.card)
            .stroke(Stroke::new(1.0, pal.border_soft))
            .corner_radius(radius(theme::R_LG))
            .inner_margin(Margin::symmetric(10, 8))
            .outer_margin(Margin { left: 30, right: 0, top: 2, bottom: 6 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if matches!(it.kind, ItemKind::Assistant { .. } | ItemKind::Reasoning { .. }) {
                    markdown::show(ui, &body, fs(13.0), pal.muted, true);
                } else {
                    egui::ScrollArea::vertical().id_salt(("out", &it.id)).max_height(260.0).show(ui, |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(body).font(theme::mono(mono_fs(12.0))).color(pal.ctx_fg)).wrap());
                    });
                }
            });
    }
    if !in_fold {
        ui.add_space(4.0);
    }
}

fn files_card(app: &mut App, ui: &mut Ui, t: &Thread) {
    let pal = p();
    let Some(g) = app.git.get(&t.id) else { return };
    let changes = g.changes.clone();
    let (n, a, d) = g.totals();
    w::card().outer_margin(Margin { left: 4, right: 4, top: 0, bottom: 12 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), Layout::left_to_right(Align::Center), |ui| {
            ui.add_space(12.0);
            w::text_medium(ui, format!("{n} file{} changed", if n == 1 { "" } else { "s" }), 12.0, pal.fg);
            w::diff_counts(ui, a, d, 12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(8.0);
                if Btn::primary("Review & ship").icon(ic::BADGE_CHECK).show(ui).clicked() {
                    app.view = View::Review;
                }
                if Btn::new("View diff").icon(ic::FILE_DIFF).show(ui).clicked() {
                    app.ui.right_open = true;
                    app.ui.right_tab = Some(RightTab::Diff);
                    app.load_diff(&t.id);
                }
            });
        });
        for (i, c) in changes.iter().take(12).enumerate() {
            let (rect, resp) = w::row_button(ui, 32.0, false, 0);
            if i > 0 || true {
                ui.painter().hline(rect.x_range(), rect.top(), Stroke::new(1.0, pal.border_faint));
            }
            let mut job = w::path_job(&c.path, 12.0, &pal);
            job.wrap.max_width = rect.width() - 120.0;
            job.wrap.max_rows = 1;
            let gal = ui.fonts_mut(|f| f.layout_job(job));
            ui.painter().galley(pos2(rect.left() + 12.0, rect.center().y - gal.size().y / 2.0), gal, pal.fg);
            let f = theme::mono(mono_fs(12.0));
            let del = if c.deletions > 0 { format!("−{}", c.deletions) } else { String::new() };
            let dw = w::galley_width(ui, &del, &f);
            ui.painter().text(pos2(rect.right() - 12.0, rect.center().y), Align2::RIGHT_CENTER, &del, f.clone(), pal.del);
            let add = if c.binary { "bin".to_string() } else { format!("+{}", c.additions) };
            ui.painter().text(pos2(rect.right() - 12.0 - dw - 6.0, rect.center().y), Align2::RIGHT_CENTER, add, f, pal.add);
            if resp.clicked() {
                app.ui.right_open = true;
                app.ui.right_tab = Some(RightTab::Diff);
                app.ui.collapsed_files.clear();
                app.load_diff(&t.id);
            }
        }
        if changes.len() > 12 {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                w::text(ui, format!("and {} more", changes.len() - 12), 12.0, pal.muted);
            });
            ui.add_space(6.0);
        }
    });
}

// ------------------------------------------------------------------------------- details

fn details_heading(ui: &mut Ui, s: &str) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        w::text_medium(ui, s, 11.0, p().muted);
    });
}

fn details_row(ui: &mut Ui, glyph: &str, label: &str, right: Option<(&str, bool)>, clickable: bool) -> egui::Response {
    let pal = p();
    let (rect, resp) = if clickable {
        w::row_button(ui, 28.0, false, theme::R_LG)
    } else {
        ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover())
    };
    w::paint_icon(ui, Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - 7.0), vec2(14.0, 14.0)), glyph, 14.0, pal.muted);
    let rw =
        right.map(|(r, mono)| w::galley_width(ui, r, &if mono { theme::mono(mono_fs(11.0)) } else { theme::ui(fs(12.0)) })).unwrap_or(0.0);
    w::paint_truncated(
        ui,
        pos2(rect.left() + 30.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::ui(fs(14.0)),
        pal.fg,
        rect.width() - 46.0 - rw,
    );
    if let Some((r, mono)) = right {
        ui.painter().text(
            pos2(rect.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            r,
            if mono { theme::mono(mono_fs(11.0)) } else { theme::ui(fs(12.0)) },
            pal.muted,
        );
    }
    resp
}

fn thread_details(app: &mut App, ui: &mut Ui, id: &str) {
    let Some(t) = app.thread(id).cloned() else { return };
    let pal = p();
    let cwd = app.cwd_of(&t);
    let scripts: Vec<(String, String)> =
        t.project_id.as_deref().and_then(|p| app.project(p)).map(|p| p.scripts.clone()).unwrap_or_default();
    let test_cmd = cwd.as_deref().and_then(crate::suggest::test_command);
    let (files, a, d) = app.git.get(id).map(|g| g.totals()).unwrap_or((0, 0, 0));
    w::popover(16).fill(if pal.dark { theme::rgba(17, 17, 17, 0.92) } else { pal.popover }).show(ui, |ui| {
        ui.set_width(280.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        Frame::new().inner_margin(Margin { left: 8, right: 8, top: 8, bottom: 10 }).show(ui, |ui| {
            details_heading(ui, "Workspace");
            ui.add_space(4.0);
            let loc = if t.is_worktree {
                "Worktree"
            } else if t.project_id.is_some() {
                "Local checkout"
            } else {
                "Scratch folder"
            };
            details_row(ui, ic::LAPTOP, loc, Some(("This PC", false)), false)
                .on_hover_text(cwd.as_ref().map(|c| c.display().to_string()).unwrap_or_default());
            let code = which("code");
            let er = details_row(ui, ic::CODE, if code.is_some() { "Open in VS Code" } else { "Open folder" }, None, true);
            if er.clicked()
                && let Some(c) = &cwd
            {
                match &code {
                    Some(exe) => {
                        let _ = std::process::Command::new(exe).arg(c).spawn();
                    }
                    None => {
                        let _ = open::that(c);
                    }
                }
            }
            if code.is_some() {
                let fr = details_row(ui, ic::FOLDER, "Open folder", None, true);
                if fr.clicked()
                    && let Some(c) = &cwd
                {
                    let _ = open::that(c);
                }
            }
            let mut all_scripts = scripts.clone();
            if let Some(tc) = &test_cmd
                && !all_scripts.iter().any(|(n, _)| n == "test")
            {
                all_scripts.push(("test".into(), tc.clone()));
            }
            for (name, cmd) in all_scripts {
                if details_row(ui, ic::PLAY, &name, Some((&provider::short(&cmd, 22), true)), true)
                    .on_hover_text(format!("Run `{cmd}` in a terminal"))
                    .clicked()
                    && let Some(c) = cwd.clone()
                {
                    app.open_terminal(c, Some(cmd.clone()));
                }
            }
            let ar = details_row(ui, ic::PLUS, "Add project script", None, true);
            if ar.clicked() {
                app.ui.settings_open = true;
                app.ui.settings_page = "projects".into();
                if let Some(pid) = &t.project_id {
                    app.ui.settings_scope = pid.clone();
                }
            }
        });
        w::hairline(ui, pal.border);
        Frame::new().inner_margin(Margin { left: 8, right: 8, top: 8, bottom: 10 }).show(ui, |ui| {
            details_heading(ui, "Version Control");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                w::icon(ui, ic::GIT_BRANCH, 14.0, pal.muted);
                w::mono_text(ui, t.branch.clone().unwrap_or_else(|| "no branch".into()), 12.0, pal.fg);
            });
            ui.add_space(4.0);
            let has_pr = t.pr_url.is_some();
            let label = if has_pr { "Open pull request" } else { "Commit & push" };
            let busy = app.ui.busy.contains(&format!("commit:{id}"));
            Frame::new().fill(pal.ctrl_bg).stroke(Stroke::new(1.0, pal.input_border)).corner_radius(radius(theme::R_LG)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let main_w = ui.available_width() - 29.0;
                    if Btn::new(if busy { "Working…" } else { label })
                        .kind(w::Kind::Subtle)
                        .icon(if has_pr { ic::GIT_PR } else { ic::CLOUD_UPLOAD })
                        .medium()
                        .min_width(main_w)
                        .enabled(!busy && (files > 0 || has_pr))
                        .show(ui)
                        .clicked()
                    {
                        if let Some(u) = &t.pr_url {
                            let _ = open::that(u);
                        } else {
                            app.view = View::Review;
                        }
                    }
                    let (sr, _) = ui.allocate_exact_size(vec2(1.0, 28.0), Sense::hover());
                    ui.painter().vline(sr.center().x, sr.y_range(), Stroke::new(1.0, pal.input_border));
                    let more = w::icon_button_sized(ui, ic::CHEVRON_DOWN, "Git action options", false, 28.0, 14.0, pal.muted);
                    egui::Popup::menu(&more).width(220.0).frame(w::popover(10).inner_margin(4)).show(|ui| {
                        ui.set_min_width(210.0);
                        if w::menu_item(ui, Some(ic::GIT_COMMIT), "Commit only", None, true).clicked() {
                            let msg = t.commit_message.clone().unwrap_or_else(|| t.title.clone());
                            app.commit_and_push(id, msg, false);
                            ui.close();
                        }
                        if w::menu_item(ui, Some(ic::CLOUD_UPLOAD), "Commit & push", None, true).clicked() {
                            let msg = t.commit_message.clone().unwrap_or_else(|| t.title.clone());
                            app.commit_and_push(id, msg, true);
                            ui.close();
                        }
                        if w::menu_item(ui, Some(ic::BADGE_CHECK), "Review & ship…", None, true).clicked() {
                            app.view = View::Review;
                            ui.close();
                        }
                    });
                });
            });
            ui.add_space(4.0);
            let cr = ui.horizontal(|ui| {
                let r = details_row(ui, ic::FILE_DIFF, "Changes", None, true);
                let rect = r.rect;
                let f = theme::mono(mono_fs(12.0));
                let del = format!("−{d}");
                let dw = w::galley_width(ui, &del, &f);
                ui.painter().text(pos2(rect.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, &del, f.clone(), pal.del);
                ui.painter().text(pos2(rect.right() - 14.0 - dw, rect.center().y), Align2::RIGHT_CENTER, format!("+{a}"), f, pal.add);
                r
            });
            if cr.inner.clicked() {
                app.ui.right_open = true;
                app.ui.right_tab = Some(RightTab::Diff);
                app.load_diff(id);
            }
            let n_cp = app.checkpoints.get(id).map(|c| c.len()).unwrap_or(0);
            if details_row(
                ui,
                ic::HISTORY,
                "Timeline",
                Some((&format!("{n_cp} checkpoint{}", if n_cp == 1 { "" } else { "s" }), false)),
                true,
            )
            .clicked()
            {
                app.ui.right_open = true;
                app.ui.right_tab = Some(RightTab::Timeline);
            }
            if details_row(ui, ic::BADGE_CHECK, "Review & ship", None, true).clicked() {
                app.view = View::Review;
            }
        });
    });
}

pub fn which(name: &str) -> Option<std::path::PathBuf> {
    let exts: &[&str] = if cfg!(windows) { &[".cmd", ".exe", ".bat"] } else { &[""] };
    std::env::var_os("PATH")
        .and_then(|p| std::env::split_paths(&p).flat_map(|d| exts.iter().map(move |e| d.join(format!("{name}{e}")))).find(|c| c.is_file()))
}

// ------------------------------------------------------------------------------- terminal

fn terminal_drawer(app: &mut App, ui: &mut Ui, id: &str) {
    let pal = p();
    let Some(cwd) = app.thread(id).and_then(|t| app.cwd_of(t)) else {
        ui.centered_and_justified(|ui| w::text(ui, "This thread has no workspace yet.", 12.0, pal.faint));
        return;
    };
    let key = cwd.to_string_lossy().to_string();
    let mine: Vec<usize> = app.terminals.iter().enumerate().filter(|(_, (k, _))| *k == key).map(|(i, _)| i).collect();
    if mine.is_empty() {
        app.open_terminal(cwd.clone(), None);
        return;
    }
    if !mine.contains(&app.ui.term_active) {
        app.ui.term_active = mine[0];
    }
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.top(), Stroke::new(1.0, pal.border));
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 36.0), Layout::left_to_right(Align::Center), |ui| {
        ui.add_space(12.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        let mut close = None;
        for (n, &i) in mine.iter().enumerate() {
            let on = i == app.ui.term_active;
            let alive = app.terminals[i].1.is_alive();
            let label = format!("Terminal {}{}", n + 1, if alive { "" } else { " (exited)" });
            if Btn::new(&label).icon(ic::TERMINAL).kind(if on { w::Kind::Subtle } else { w::Kind::Ghost }).selected(on).show(ui).clicked() {
                app.ui.term_active = i;
            }
            if on && w::icon_button_sized(ui, ic::X, "Close this terminal", false, 20.0, 12.0, pal.muted).clicked() {
                close = Some(i);
            }
        }
        if w::icon_button_sized(ui, ic::PLUS, "New terminal", false, 24.0, 14.0, pal.muted).clicked() {
            app.open_terminal(cwd.clone(), None);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(8.0);
            if w::icon_button_sized(ui, ic::X, "Close terminal drawer", false, 24.0, 14.0, pal.muted).clicked() {
                app.ui.terminal_open = false;
            }
        });
        if let Some(i) = close {
            let (_, mut t) = app.terminals.remove(i);
            t.kill();
            app.ui.term_active = 0;
        }
    });
    let active = app.ui.term_active;
    if let Some((_, term)) = app.terminals.get_mut(active) {
        let colors = if pal.dark {
            crate::terminal::TerminalColors::default()
        } else {
            crate::terminal::TerminalColors { background: pal.bg, foreground: pal.fg, cursor: pal.fg, ..Default::default() }
        };
        Frame::new().inner_margin(Margin { left: 12, right: 8, top: 0, bottom: 8 }).show(ui, |ui| {
            crate::terminal::terminal_view(ui, term, theme::mono(mono_fs(12.0)), &colors);
        });
    }
}

// ------------------------------------------------------------------------------- right panel

pub fn right_panel(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let Some(id) = app.current.clone() else {
        ui.centered_and_justified(|ui| w::text(ui, "Open a thread to see its changes.", 12.0, pal.faint));
        return;
    };
    let tab = app.ui.right_tab.unwrap_or(RightTab::Diff);
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 52.0), Layout::left_to_right(Align::Center), |ui| {
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        for (t, label, glyph) in [(RightTab::Diff, "Diff", ic::FILE_DIFF), (RightTab::Timeline, "Timeline", ic::HISTORY)] {
            if Btn::new(label).icon(glyph).medium().kind(w::Kind::Ghost).selected(tab == t).show(ui).clicked() {
                app.ui.right_tab = Some(t);
                if t == RightTab::Diff {
                    app.load_diff(&id);
                }
            }
        }
    });
    match tab {
        RightTab::Diff => diff_tab(app, ui, &id),
        RightTab::Timeline => timeline_tab(app, ui, &id),
    }
}

fn diff_tab(app: &mut App, ui: &mut Ui, id: &str) {
    let pal = p();
    if app.git.get(id).is_none_or(|g| g.diff.is_none() && !g.diff_loading) {
        app.load_diff(id);
    }
    let t = app.thread(id).cloned();
    let split = app.ui.diff_split.unwrap_or(app.settings.diff_layout == "Split");
    let wrap = app.ui.diff_wrap.unwrap_or(app.settings.word_wrap);
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 28.0), Layout::left_to_right(Align::Center), |ui| {
        ui.add_space(16.0);
        let base_label = app.git.get(id).and_then(|g| g.diff_base.clone()).map(|b| b.1).unwrap_or_else(|| "Since thread start".into());
        w::text_medium(ui, &base_label, 14.0, pal.fg);
        if let Some(b) = t.as_ref().and_then(|t| t.base_rev.clone()) {
            w::mono_text(ui, format!("{} → working tree", &b[..b.len().min(7)]), 12.0, pal.muted);
        }
        if app.git.get(id).is_some_and(|g| g.diff_base.is_some()) && Btn::ghost("Reset").show(ui).clicked() {
            if let Some(g) = app.git.get_mut(id) {
                g.diff_base = None;
                g.diff_loading = false;
            }
            app.load_diff(id);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(12.0);
            if w::icon_button_sized(ui, ic::PILCROW, "Hide whitespace changes", app.settings.hide_whitespace, 28.0, 14.0, pal.muted)
                .clicked()
            {
                app.settings.hide_whitespace = !app.settings.hide_whitespace;
            }
            if w::icon_button_sized(ui, ic::TEXT_WRAP, "Wrap lines", wrap, 28.0, 14.0, pal.muted).clicked() {
                app.ui.diff_wrap = Some(!wrap);
            }
            Frame::new().fill(pal.ctrl_bg).stroke(Stroke::new(1.0, pal.border)).corner_radius(radius(theme::R_LG)).inner_margin(2).show(
                ui,
                |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if w::icon_button_sized(ui, ic::COLUMNS2, "Split diff view", split, 22.0, 14.0, pal.muted).clicked() {
                        app.ui.diff_split = Some(true);
                    }
                    if w::icon_button_sized(ui, ic::ROWS3, "Stacked diff view", !split, 22.0, 14.0, pal.muted).clicked() {
                        app.ui.diff_split = Some(false);
                    }
                },
            );
        });
    });
    ui.add_space(8.0);
    let Some(doc) = app.git.get(id).and_then(|g| g.diff.clone()) else {
        let msg = app.git.get(id).and_then(|g| g.error.clone()).unwrap_or_else(|| "Loading changes…".into());
        ui.centered_and_justified(|ui| w::text(ui, msg, 12.0, pal.faint));
        return;
    };
    if doc.files.is_empty() {
        ui.centered_and_justified(|ui| w::text(ui, "No changes yet.", 12.0, pal.faint));
        return;
    }
    let opts = diff::Opts {
        split,
        wrap,
        only_file: None,
        file_headers: true,
        revert: false,
        comments: false,
        hide_ws: app.settings.hide_whitespace,
    };
    let viewed = app.viewed.get(id).cloned().unwrap_or_default();
    let mut draft = String::new();
    let acts = diff::show(ui, &format!("rdiff{id}"), &doc, &opts, &app.ui.collapsed_files.clone(), Some(&viewed), &[], None, &mut draft);
    for a in acts {
        match a {
            diff::Action::ToggleCollapse(p) => {
                if !app.ui.collapsed_files.remove(&p) {
                    app.ui.collapsed_files.insert(p);
                }
            }
            diff::Action::ToggleViewed(fi) => {
                let f = &doc.files[fi];
                let fp = doc.fingerprints[fi].clone();
                let on = viewed.get(&f.path) == Some(&fp);
                app.set_viewed(id, &f.path, (!on).then_some(fp));
            }
            _ => {}
        }
    }
}

fn timeline_tab(app: &mut App, ui: &mut Ui, id: &str) {
    let pal = p();
    let cps = app.checkpoints.get(id).cloned().unwrap_or_default();
    egui::ScrollArea::vertical().id_salt(("cps", id)).auto_shrink([false, false]).show(ui, |ui| {
        Frame::new().inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 16 }).show(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(if app.settings.checkpoints { "Kiln saves a checkpoint of the working tree before every agent turn (tracked and untracked files; ignored files are not captured). Pick one to see what changed since, restore it, or fork from it." } else { "Checkpoints are turned off in Settings → SnapShots." }).font(theme::ui(fs(12.0))).color(pal.muted)).wrap());
            ui.add_space(12.0);
            if cps.is_empty() {
                w::text(ui, "No checkpoints yet.", 12.0, pal.faint);
                return;
            }
            let sel = app.ui.selected_checkpoint.unwrap_or(cps.len()).min(cps.len());
            for (i, cp) in cps.iter().enumerate().chain(std::iter::once((cps.len(), &Checkpoint { id: "now".into(), thread_id: id.into(), sha: String::new(), title: "Now".into(), detail: "Current working tree".into(), additions: 0, deletions: 0, created_at: now() }))) {
                let on = i == sel;
                let last = i == cps.len();
                w::row_top(ui, |ui| {
                    let (gr, _) = ui.allocate_exact_size(vec2(14.0, 52.0), Sense::hover());
                    let c = pos2(gr.center().x, gr.top() + 17.0);
                    if !last {
                        ui.painter().vline(c.x, (c.y + 6.0)..=gr.bottom() + 6.0, Stroke::new(1.0, theme::rgba(255, 255, 255, 0.1)));
                    }
                    if on {
                        ui.painter().circle_filled(c, 8.0, theme::tint(pal.accent, 0.3));
                    }
                    ui.painter().circle_filled(c, 5.0, if last { pal.accent } else if on { pal.fg } else { theme::hex(0x525252) });
                    let fr = Frame::new().fill(if on { theme::rgba(255, 255, 255, 0.04) } else { egui::Color32::TRANSPARENT }).stroke(Stroke::new(1.0, if on { theme::rgba(255, 255, 255, 0.14) } else { egui::Color32::TRANSPARENT })).corner_radius(radius(10)).inner_margin(Margin::symmetric(10, 7)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            w::text_medium(ui, &cp.title, 13.0, pal.fg);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                w::text(ui, if last { "Now".to_string() } else { w::ago(cp.created_at) }, 12.0, pal.muted);
                            });
                        });
                        ui.horizontal(|ui| {
                            w::text(ui, &cp.detail, 12.0, pal.muted);
                            if cp.additions + cp.deletions > 0 {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    w::diff_counts(ui, cp.additions, cp.deletions, 12.0);
                                });
                            }
                        });
                    });
                    if fr.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        app.ui.selected_checkpoint = Some(i);
                    }
                });
            }
            ui.add_space(6.0);
            if sel < cps.len() {
                let cp = cps[sel].clone();
                w::card().inner_margin(12).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    w::text_medium(ui, &cp.title, 13.0, pal.fg);
                    ui.add(egui::Label::new(egui::RichText::new("Restoring puts tracked and untracked files back to this checkpoint. Kiln saves the current state as a checkpoint first, so you can undo.").font(theme::ui(fs(12.0))).color(pal.muted)).wrap());
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        if Btn::new("Diff since here").icon(ic::FILE_DIFF).show(ui).clicked() {
                            if let Some(g) = app.git.get_mut(id) {
                                g.diff_base = Some((cp.sha.clone(), format!("Since “{}”", provider::short(&cp.title, 30))));
                                g.diff_loading = false;
                            }
                            app.ui.right_tab = Some(RightTab::Diff);
                            app.load_diff(id);
                        }
                        if Btn::new("Restore files").icon(ic::ROTATE_CCW).show(ui).clicked() {
                            app.preview_restore(id, cp.clone());
                        }
                        if Btn::new("Fork a thread from here").icon(ic::GIT_FORK).show(ui).clicked() {
                            app.fork_from_checkpoint(id, &cp);
                        }
                    });
                });
            }
        });
    });
}
