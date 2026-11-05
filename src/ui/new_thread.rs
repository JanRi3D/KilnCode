//! New thread: project, prompt, attachments, provider/model, workspace, base branch,
//! single-agent vs. compare runs, and starters taken from the actual repository.

use super::composer;
use crate::app::{App, View};
use crate::icons as ic;
use crate::model::ProviderKind;
use crate::theme::{self, fs, mono_fs, p, radius};
use crate::widgets::{self as w, Btn};
use egui::{Align, Frame, Layout, Margin, Sense, Stroke, Ui, vec2};

pub fn show(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let right_pad = if app.ui.right_open { 20.0 } else { 152.0 };
    let proj_name = app.draft.project.as_deref().and_then(|id| app.project(id)).map(|p| p.name.clone());
    super::header(ui, right_pad, |ui| {
        if !app.ui.sidebar_open && w::icon_button(ui, ic::PANEL_LEFT, "Show sidebar", false).clicked() {
            app.ui.sidebar_open = true;
        }
        if let Some(n) = &proj_name {
            super::crumb_project(ui, n);
        }
        w::text_medium(ui, "New thread", 14.0, pal.fg);
    });
    if let Some(pid) = app.draft.project.clone() {
        app.load_starters(&pid);
    }
    let narrow = ui.available_width() < 760.0;
    let lane_pad = if narrow { 12.0 } else { 48.0 };
    egui::ScrollArea::vertical().id_salt("new_thread").auto_shrink([false, false]).show(ui, |ui| {
        let avail = ui.available_width();
        let lane = (avail - 2.0 * lane_pad).min(theme::LANE_MAX);
        let top_space = ((ui.available_height() - 520.0) / 2.0).max(24.0);
        ui.add_space(top_space);
        ui.horizontal(|ui| {
            ui.add_space((avail - lane) / 2.0);
            ui.vertical(|ui| {
                ui.set_width(lane);
                heading(app, ui);
                ui.add_space(16.0);
                editor(app, ui);
                footer(app, ui);
                if app.draft.compare {
                    runs_editor(app, ui);
                }
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    let label = if app.draft.no_project { "or start in a project" } else { "or start without a project" };
                    let r = ui.add(
                        egui::Label::new(egui::RichText::new(label).font(theme::ui(fs(14.0))).color(pal.faint).underline())
                            .sense(Sense::click()),
                    );
                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        app.draft.no_project = !app.draft.no_project;
                    }
                });
                if app.draft.no_project {
                    ui.add_space(6.0);
                    ui.vertical_centered(|ui| {
                        w::text(ui, "The agent works in an empty scratch folder in Kiln's data directory.", 12.0, pal.muted)
                    });
                }
                ui.add_space(40.0);
                starters(app, ui, lane);
            });
        });
        ui.add_space(40.0);
    });
}

fn heading(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let font = theme::ui(fs(30.0));
    ui.vertical_centered(|ui| {
        if app.projects.is_empty() {
            ui.label(egui::RichText::new("Add a project to get started").font(font.clone()).color(pal.fg));
            ui.add_space(12.0);
            if Btn::primary("Add project").icon(ic::FOLDER_PLUS).medium().show(ui).clicked() {
                app.pick_project_folder();
            }
            return;
        }
        if app.draft.no_project {
            ui.label(egui::RichText::new("What should we build?").font(font).color(pal.fg));
            return;
        }
        let name =
            app.draft.project.as_deref().and_then(|id| app.project(id)).map(|p| p.name.clone()).unwrap_or_else(|| "a project".into());
        ui.horizontal(|ui| {
            let total = w::galley_width(ui, &format!("What should we build in {name}?"), &font);
            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(egui::RichText::new("What should we build in ").font(font.clone()).color(pal.fg));
            let r = ui
                .add(egui::Label::new(egui::RichText::new(&name).font(font.clone()).color(pal.fg).underline()).sense(Sense::click()))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            ui.label(egui::RichText::new("?").font(font.clone()).color(pal.fg));
            egui::Popup::menu(&r).width(260.0).frame(w::popover(10).inner_margin(4)).show(|ui| {
                ui.set_min_width(250.0);
                let mut projects: Vec<(String, String, bool)> =
                    app.projects.iter().filter(|p| !p.archived).map(|p| (p.id.clone(), p.name.clone(), p.pinned)).collect();
                projects.sort_by_key(|p| !p.2);
                for (id, n, _) in projects {
                    if w::menu_item(ui, (app.draft.project.as_deref() == Some(id.as_str())).then_some(ic::CHECK), &n, None, true).clicked()
                    {
                        app.draft.project = Some(id.clone());
                        app.draft.base_branch.clear();
                        app.save_ui();
                        ui.close();
                    }
                }
                w::menu_separator(ui);
                if w::menu_item(ui, Some(ic::FOLDER_PLUS), "Add project…", None, true).clicked() {
                    app.pick_project_folder();
                    ui.close();
                }
            });
        });
    });
}

fn editor(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let te_id = ui.scope_id().with("new_thread_editor");
    let (send, _) = composer::send_keys(app, ui, te_id);
    Frame::new().fill(if pal.dark { theme::rgba(17, 17, 17, 0.8) } else { pal.raised }).stroke(Stroke::new(1.0, if pal.dark { theme::rgba(255, 255, 255, 0.05) } else { pal.border })).corner_radius(radius(theme::R_2XL)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        Frame::new().inner_margin(Margin { left: 16, right: 16, top: 16, bottom: 8 }).show(ui, |ui| {
            if !app.draft.attachments.is_empty() {
                let mut remove = None;
                ui.horizontal_wrapped(|ui| {
                    for (i, a) in app.draft.attachments.iter().enumerate() {
                        Frame::new().fill(pal.ctrl_bg).stroke(Stroke::new(1.0, pal.input_border)).corner_radius(radius(theme::R_LG)).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
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
                if let Some(i) = remove {
                    app.draft.attachments.remove(i);
                }
                ui.add_space(6.0);
            }
            let r = ui.add(egui::TextEdit::multiline(&mut app.draft.prompt).id(te_id).hint_text(egui::RichText::new("Ask anything. Describe the change, paste an error, or pick a starter below.").color(pal.faint)).frame(Frame::NONE).font(theme::ui(fs(14.0))).desired_rows(3).desired_width(f32::INFINITY));
            if std::mem::take(&mut app.ui.focus_composer) || (!r.has_focus() && app.draft.prompt.is_empty() && !app.ui.palette_open && !app.ui.settings_open && ui.memory(|m| m.focused().is_none())) {
                r.request_focus();
            }
        });
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), Layout::left_to_right(Align::Center), |ui| {
            ui.add_space(8.0);
            if w::icon_button_sized(ui, ic::PAPERCLIP, "Attach files", false, 28.0, 16.0, pal.muted).clicked() {
                app.pick_attachments(None);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(8.0);
                ui.spacing_mut().item_spacing.x = 8.0;
                let ok = !app.draft.prompt.trim().is_empty() && (app.draft.project.is_some() || app.draft.no_project);
                if composer::send_button(ui, ok).on_hover_text("Start thread").clicked() || (send && ok) {
                    app.start_from_draft();
                }
                if !app.draft.compare {
                    composer::model_picker(app, ui, None, app.draft.provider);
                }
                let n = if app.draft.compare { app.draft.runs.len() } else { 1 };
                let label = if app.draft.compare { format!("Compare · {n} agents") } else { "1 agent".into() };
                let on = app.draft.compare;
                let b = Btn::new(&label).icon(ic::GIT_COMPARE).medium().kind(w::Kind::Subtle).tooltip("Send this prompt to several agents at once, each in its own worktree from the same base commit, and compare the results").selected(on).show(ui);
                if b.clicked() {
                    toggle_compare(app);
                }
            });
        });
    });
}

fn toggle_compare(app: &mut App) {
    app.draft.compare = !app.draft.compare;
    if app.draft.compare && app.draft.runs.len() < 2 {
        let mut runs = Vec::new();
        for k in ProviderKind::ALL {
            if app.provider_enabled(k) && app.providers.get(&k).is_some_and(|s| s.executable.is_some()) {
                runs.push((k, app.default_model(k).0));
            }
        }
        if runs.len() < 2 {
            let k = runs.first().map(|r| r.0).unwrap_or(ProviderKind::Claude);
            let models = app.models_for(k);
            runs = models.iter().filter(|m| m.id != "default").take(2).map(|m| (k, m.id.clone())).collect();
        }
        app.draft.runs = runs;
    }
}

fn runs_editor(app: &mut App, ui: &mut Ui) {
    let pal = p();
    ui.add_space(8.0);
    w::card().inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        w::text_medium(ui, "Runs · each gets its own worktree from the same base commit", 12.0, pal.muted);
        ui.add_space(6.0);
        let mut remove = None;
        for (i, (k, m)) in app.draft.runs.clone().iter().enumerate() {
            ui.horizontal(|ui| {
                w::badge(ui, &((b'A' + i as u8) as char).to_string(), pal.fg, pal.hover);
                w::text(ui, format!("{} · {}", k.label(), app.model_label(*k, m)), 13.0, pal.fg);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if app.draft.runs.len() > 2 && w::icon_button_sized(ui, ic::X, "Remove run", false, 20.0, 12.0, pal.muted).clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove {
            app.draft.runs.remove(i);
        }
        if app.draft.runs.len() < 4 {
            let r = Btn::ghost("Add run").icon(ic::PLUS).show(ui);
            egui::Popup::menu(&r).width(280.0).frame(w::popover(10).inner_margin(4)).show(|ui| {
                ui.set_min_width(270.0);
                let kinds: Vec<ProviderKind> = ProviderKind::ALL.into_iter().filter(|k| app.provider_enabled(*k)).collect();
                for k in kinds {
                    w::menu_heading(ui, k.label());
                    for m in app.models_for(k) {
                        if w::menu_item(ui, None, if m.display.is_empty() { &m.id } else { &m.display }, None, true).clicked() {
                            app.draft.runs.push((k, m.id.clone()));
                            ui.close();
                        }
                    }
                }
            });
        }
    });
}

fn footer(app: &mut App, ui: &mut Ui) {
    let pal = p();
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        if app.draft.no_project {
            w::icon(ui, ic::FOLDER, 12.0, pal.muted);
            w::text(ui, "Scratch folder", 12.0, pal.muted);
            return;
        }
        let wt = app.draft.worktree || app.draft.compare;
        let r = Btn::ghost(if wt { "New worktree" } else { "Current checkout" })
            .icon(if wt { ic::FOLDER_GIT } else { ic::FOLDER })
            .trailing(ic::CHEVRON_DOWN)
            .show(ui);
        egui::Popup::menu(&r).width(260.0).frame(w::popover(10).inner_margin(4)).show(|ui| {
            ui.set_min_width(250.0);
            if w::menu_item(ui, wt.then_some(ic::CHECK), "New worktree", None, true).clicked() {
                app.draft.worktree = true;
                ui.close();
            }
            if !app.draft.compare && w::menu_item(ui, (!wt).then_some(ic::CHECK), "Current checkout", None, true).clicked() {
                app.draft.worktree = false;
                ui.close();
            }
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(if app.draft.compare {
                            "Compare runs always use separate worktrees."
                        } else {
                            "A worktree isolates the agent's changes on its own branch; your checkout and uncommitted work stay untouched."
                        })
                        .font(theme::ui(fs(11.0)))
                        .color(pal.faint),
                    )
                    .wrap(),
                );
            });
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(8.0);
            let pid = app.draft.project.clone().unwrap_or_default();
            let (list, def) = app.branches.get(&pid).cloned().unwrap_or_default();
            let base = if app.draft.base_branch.is_empty() { def.clone() } else { app.draft.base_branch.clone() };
            let label = if wt {
                let slug = crate::git::slugify(&crate::app::title_from(&app.draft.prompt), 40);
                let prefix = app.settings.branch_prefix.trim().trim_end_matches('/');
                if app.draft.prompt.trim().is_empty() {
                    format!("From {base}")
                } else if prefix.is_empty() {
                    format!("{slug} ← {base}")
                } else {
                    format!("{prefix}/{slug} ← {base}")
                }
            } else {
                base.clone()
            };
            if base.is_empty() {
                // Branches are still loading (or the repo has none yet): show nothing rather than a bare "From".
                return;
            }
            let r = Btn::ghost(&label).icon(ic::GIT_BRANCH).show(ui);
            if wt {
                egui::Popup::menu(&r).width(280.0).frame(w::popover(10).inner_margin(4)).show(|ui| {
                    ui.set_min_width(270.0);
                    w::menu_heading(ui, "Base branch");
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        if list.is_empty() {
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                w::text(ui, "No branches found yet.", 12.0, pal.faint);
                            });
                        }
                        for b in &list {
                            if w::menu_item(ui, (*b == base).then_some(ic::CHECK), b, (*b == def).then_some("default"), true).clicked() {
                                app.draft.base_branch = b.clone();
                                ui.close();
                            }
                        }
                    });
                });
            }
        });
    });
}

fn starters(app: &mut App, ui: &mut Ui, lane: f32) {
    let pal = p();
    let Some(pid) = app.draft.project.clone() else { return };
    if app.draft.no_project {
        return;
    }
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        w::text_medium(ui, "From your repo", 12.0, pal.muted);
    });
    ui.add_space(8.0);
    match app.starters.get(&pid).cloned().flatten() {
        None => {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                w::spinner(ui, 12.0, pal.muted);
                w::text(ui, "Looking at the repository…", 12.0, pal.faint);
            });
        }
        Some(list) if list.is_empty() => {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.add(egui::Label::new(egui::RichText::new("Nothing to suggest: no TODOs, uncommitted changes, open GitHub issues or failing checks were found. (GitHub suggestions need the gh CLI signed in.)").font(theme::ui(fs(12.0))).color(pal.faint)).wrap());
            });
        }
        Some(list) => {
            let cols = if lane >= 608.0 { 2 } else { 1 };
            let cw = (lane - 8.0 * (cols as f32 - 1.0)) / cols as f32;
            for chunk in list.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    for st in chunk {
                        let r = Frame::new()
                            .fill(pal.card)
                            .stroke(Stroke::new(1.0, pal.border_soft))
                            .corner_radius(radius(theme::R_XL))
                            .inner_margin(Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.set_width(cw - 26.0);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.horizontal(|ui| {
                                        let (d, _) = ui.allocate_exact_size(vec2(6.0, 6.0), Sense::hover());
                                        ui.painter().circle_filled(d.center(), 3.0, st.tone);
                                        w::text(ui, &st.tag, 12.0, st.tone);
                                    });
                                    w::text_medium(ui, &st.title, 14.0, pal.fg);
                                    ui.label(egui::RichText::new(&st.meta).font(theme::mono(mono_fs(11.0))).color(pal.muted));
                                });
                            });
                        let resp = r.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.hovered() {
                            ui.painter().rect_stroke(
                                r.response.rect,
                                radius(theme::R_XL),
                                Stroke::new(1.0, pal.input_border),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if resp.clicked() {
                            app.draft.prompt = st.prompt.clone();
                            app.ui.focus_composer = true;
                        }
                    }
                });
                ui.add_space(8.0);
            }
        }
    }
    let _ = View::NewThread;
}
