//! Root layout: sidebar, center view, right panel, terminal drawer and overlays.

pub mod board;
pub mod compare;
pub mod composer;
pub mod diff;
pub mod markdown;
pub mod new_thread;
pub mod review;
pub mod settings;
pub mod sidebar;
pub mod thread;

use crate::app::{App, Confirm, RightTab, View};
use crate::icons as ic;
use crate::model::RunState;
use crate::theme::{self, Palette, fs, p, radius};
use crate::widgets::{self as w, Btn};
use egui::{Align, Align2, Frame, Layout, Margin, Rect, Sense, Stroke, Ui, vec2};

pub fn root(app: &mut App, ui: &mut Ui) {
    app.focused = ui.input(|i| i.viewport().focused.unwrap_or(true));
    app.system_dark = ui.ctx().system_theme().map(|t| t == egui::Theme::Dark).unwrap_or(true);
    apply_theme(app, ui.ctx());
    app.pump();
    keybindings(app, ui);
    let pal = p();

    if app.ui.sidebar_open {
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(320.0)
            .size_range(248.0..=460.0)
            .frame(Frame::new().fill(pal.bg))
            .show(ui, |ui| sidebar::show(app, ui));
    }

    let thread_like = matches!(app.view, View::Thread | View::NewThread);
    if thread_like && app.ui.right_open {
        egui::Panel::right("right_panel")
            .resizable(true)
            .default_size(540.0)
            .size_range(320.0..=900.0)
            .frame(Frame::new().fill(pal.bg))
            .show(ui, |ui| thread::right_panel(app, ui));
    }

    egui::CentralPanel::default().frame(Frame::new().fill(pal.bg)).show(ui, |ui| match app.view.clone() {
        View::Thread => thread::show(app, ui),
        View::NewThread => new_thread::show(app, ui),
        View::Compare(g) => compare::show(app, ui, &g),
        View::Review => review::show(app, ui),
        View::Board => board::show(app, ui),
    });

    if thread_like {
        panel_toggles(app, ui);
    }
    if app.demo {
        demo_banner(ui);
    }
    toasts(app, ui);
    if app.ui.palette_open {
        palette(app, ui);
    }
    if app.ui.settings_open {
        settings::show(app, ui);
    }
    confirm(app, ui);
}

pub fn apply_theme(app: &App, ctx: &egui::Context) {
    let dark = app.settings.is_dark(app.system_dark);
    let accent = theme::accent_for_theme(&app.settings.theme_name, dark);
    let blue_orange = app.settings.diff_colors == "Blue & orange";
    let pal =
        if dark { Palette::dark(accent, blue_orange) } else { Palette::light(accent, blue_orange) }.with_contrast(app.settings.contrast);
    let prev = p();
    theme::set_font_sizes(app.settings.ui_font_size as f32, app.settings.mono_font_size as f32);
    let key = (dark, accent, blue_orange, app.settings.contrast, app.settings.ui_font_size, app.settings.mono_font_size);
    let id = egui::Id::unique("kiln_style_key");
    let last: Option<String> = ctx.data(|d| d.get_temp(id));
    let k = format!("{key:?}");
    if last.as_deref() != Some(k.as_str()) || prev.dark != pal.dark {
        theme::set_palette(pal);
        theme::apply_style(ctx, &pal);
        ctx.data_mut(|d| d.insert_temp(id, k));
    } else {
        theme::set_palette(pal);
    }
}

fn chord_pressed(ui: &Ui, chord: &str) -> bool {
    let Some(c) = crate::settings::parse_chord(chord) else { return false };
    ui.input_mut(|i| {
        let m = egui::Modifiers { alt: c.alt, ctrl: c.ctrl, shift: c.shift, mac_cmd: false, command: c.ctrl };
        i.consume_key(m, c.key)
    })
}

fn keybindings(app: &mut App, ui: &mut Ui) {
    if app.ui.capture_key.is_some() {
        return;
    }
    let kb = app.settings.keybindings.clone();
    let get = |k: &str| kb.get(k).cloned().unwrap_or_default();
    if chord_pressed(ui, &get("palette")) {
        app.ui.palette_open = !app.ui.palette_open;
        app.ui.palette_query.clear();
    }
    if chord_pressed(ui, &get("settings")) {
        app.ui.settings_open = !app.ui.settings_open;
    }
    if app.ui.settings_open || app.ui.palette_open {
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            app.ui.settings_open = false;
            app.ui.palette_open = false;
        }
        return;
    }
    if chord_pressed(ui, &get("new_thread")) {
        app.view = View::NewThread;
        app.ui.focus_composer = true;
    }
    if chord_pressed(ui, &get("toggle_sidebar")) {
        app.ui.sidebar_open = !app.ui.sidebar_open;
    }
    if chord_pressed(ui, &get("board")) {
        app.view = if app.view == View::Board { View::Thread } else { View::Board };
    }
    if chord_pressed(ui, &get("toggle_right")) {
        app.ui.right_open = !app.ui.right_open;
    }
    if chord_pressed(ui, &get("toggle_terminal")) {
        app.ui.terminal_open = !app.ui.terminal_open;
    }
    if chord_pressed(ui, &get("toggle_details")) {
        app.ui.details_open = !app.ui.details_open;
    }
    let order = sidebar::visible_order(app);
    let cur = app.current.clone();
    let pos = cur.as_deref().and_then(|c| order.iter().position(|x| x == c));
    if (chord_pressed(ui, &get("next_thread")) || chord_pressed(ui, "Alt+Down") && get("next_needs_you") != "Alt+Down")
        && let Some(id) = pos.map(|p| (p + 1) % order.len().max(1)).and_then(|p| order.get(p)).or(order.first()).cloned()
    {
        app.open_thread(&id);
    }
    if chord_pressed(ui, &get("prev_thread"))
        && let Some(id) = pos.map(|p| (p + order.len() - 1) % order.len().max(1)).and_then(|p| order.get(p)).or(order.last()).cloned()
    {
        app.open_thread(&id);
    }
    if chord_pressed(ui, &get("next_needs_you")) {
        let need: Vec<String> =
            app.threads.iter().filter(|t| !t.archived && t.state == RunState::NeedsApproval).map(|t| t.id.clone()).collect();
        if let Some(id) = need.iter().skip_while(|x| Some(x.as_str()) != cur.as_deref()).nth(1).or(need.first()).cloned() {
            app.open_thread(&id);
        }
    }
    if let Some(id) = cur
        && app.view == View::Thread
    {
        if chord_pressed(ui, &get("archive_thread")) {
            app.set_archived(&id, true);
        }
        if app.approvals.get(&id).is_some_and(|a| !a.is_empty())
            && chord_pressed(ui, &get("approve"))
            && let Some(a) = app.approvals.get(&id).and_then(|a| a.first()).cloned()
        {
            app.decide(&id, &a.request_id, crate::provider::Decision::Allow, None);
        }
        let running = app.thread(&id).is_some_and(|t| t.state.is_active());
        if running && chord_pressed(ui, &get("interrupt")) {
            app.interrupt(&id);
        }
    }
}

/// Thread details / timeline / terminal / right-panel toggles, pinned to the window's top-right.
fn panel_toggles(app: &mut App, ui: &mut Ui) {
    let screen = ui.ctx().content_rect();
    egui::Area::new(egui::Id::unique("panel_toggles"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(screen.right() - 12.0 - 4.0 * 28.0 - 3.0 * 4.0, 12.0))
        .show(ui.ctx(), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let fg = p().fg.gamma_multiply(0.8);
                if app.view == View::Thread
                    && w::icon_button_sized(ui, ic::SQUARE_MENU, "Toggle thread details", app.ui.details_open, 28.0, 16.0, fg).clicked()
                {
                    app.ui.details_open = !app.ui.details_open;
                    app.save_ui();
                }
                let tl = app.ui.right_open && app.ui.right_tab == Some(RightTab::Timeline);
                if w::icon_button_sized(ui, ic::HISTORY, "Toggle turn timeline", tl, 28.0, 16.0, fg).clicked() {
                    app.ui.right_open = !tl;
                    app.ui.right_tab = Some(RightTab::Timeline);
                }
                if w::icon_button_sized(ui, ic::PANEL_BOTTOM, "Toggle terminal drawer", app.ui.terminal_open, 28.0, 16.0, fg).clicked() {
                    app.ui.terminal_open = !app.ui.terminal_open;
                }
                if w::icon_button_sized(ui, ic::PANEL_RIGHT, "Toggle right panel", app.ui.right_open, 28.0, 16.0, fg).clicked() {
                    app.ui.right_open = !app.ui.right_open;
                    app.save_ui();
                }
            });
        });
}

fn demo_banner(ui: &mut Ui) {
    let screen = ui.ctx().content_rect();
    egui::Area::new(egui::Id::unique("demo_banner")).order(egui::Order::Foreground).anchor(Align2::RIGHT_BOTTOM, vec2(-12.0, -8.0)).show(
        ui.ctx(),
        |ui| {
            let _ = screen;
            Frame::new()
                .fill(theme::tint(p().warning, 0.16))
                .stroke(Stroke::new(1.0, theme::tint(p().warning, 0.4)))
                .corner_radius(radius(255))
                .inner_margin(Margin::symmetric(12, 4))
                .show(ui, |ui| {
                    w::text_medium(ui, "Demo mode · sample data · agents don't run", 12.0, p().warning);
                });
        },
    );
}

fn toasts(app: &mut App, ui: &mut Ui) {
    let toasts: Vec<(String, bool, bool)> = app.toasts.iter().map(|t| (t.text.clone(), t.ok, t.link_rules)).collect();
    if toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::unique("toasts"))
        .order(egui::Order::Tooltip)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 60.0))
        .interactable(true)
        .show(ui.ctx(), |ui| {
            for (i, (text, ok, link)) in toasts.iter().enumerate() {
                let r = w::popover(255).inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
                    ui.set_max_width(640.0);
                    ui.horizontal(|ui| {
                        w::icon(ui, if *ok { ic::CHECK } else { ic::ALERT }, 14.0, if *ok { p().success } else { p().danger });
                        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::ui(fs(13.0))).color(p().fg)).wrap());
                        if *link && ui.link("Manage rules").clicked() {
                            app.ui.settings_open = true;
                            app.ui.settings_page = "approvals".into();
                        }
                    });
                });
                if r.response.interact(Sense::click()).clicked()
                    && let Some(t) = app.toasts.get_mut(i)
                {
                    t.until = 0.0;
                }
                ui.add_space(6.0);
            }
        });
}

/// Modal confirmation for consequential, user-initiated actions.
fn confirm(app: &mut App, ui: &mut Ui) {
    let Some(c) = app.confirm.clone() else { return };
    let mut close = false;
    let modal = egui::Modal::new(egui::Id::unique("confirm")).frame(w::popover(16).inner_margin(20)).backdrop_color(p().backdrop).show(ui.ctx(), |ui| {
        ui.set_width(460.0);
        let pal = p();
        match &c {
            Confirm::RestoreCheckpoint { thread, cp, changes } => {
                w::text_semibold(ui, format!("Restore files to “{}”?", cp.title), 16.0, pal.fg);
                ui.add_space(6.0);
                w::text_wrap(ui, "Kiln first saves the current state as a new checkpoint, so you can come back. Ignored files (like .env and build output) are not captured and won't change. Git history and the branch are untouched.", 13.0, pal.muted);
                ui.add_space(8.0);
                if changes.is_empty() {
                    w::text(ui, "Nothing would change: the files already match this checkpoint.", 13.0, pal.muted);
                } else {
                    w::text_medium(ui, format!("{} file{} will change:", changes.len(), if changes.len() == 1 { "" } else { "s" }), 13.0, pal.fg);
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        for ch in changes {
                            ui.horizontal(|ui| {
                                w::text(ui, change_word(ch.kind), 12.0, pal.muted);
                                ui.add(egui::Label::new(w::path_job(&ch.path, 12.0, &pal)));
                            });
                        }
                    });
                }
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Btn::primary("Restore files").medium().enabled(!changes.is_empty()).show(ui).clicked() {
                        app.restore(thread, cp.clone());
                        close = true;
                    }
                    if Btn::ghost("Cancel").medium().show(ui).clicked() {
                        close = true;
                    }
                });
            }
            Confirm::RemoveWorktree { thread } => {
                w::text_semibold(ui, "Remove this worktree?", 16.0, pal.fg);
                ui.add_space(6.0);
                w::text_wrap(ui, "Git refuses to remove a worktree with uncommitted changes, so local work is never deleted. The branch and the thread history are kept.", 13.0, pal.muted);
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Btn::new("Remove worktree").kind(w::Kind::Danger).medium().show(ui).clicked() {
                        app.remove_worktree(thread);
                        close = true;
                    }
                    if Btn::ghost("Cancel").medium().show(ui).clicked() {
                        close = true;
                    }
                });
            }
            Confirm::DeleteThread { thread } => {
                w::text_semibold(ui, "Delete this thread?", 16.0, pal.fg);
                ui.add_space(6.0);
                w::text_wrap(ui, "This deletes Kiln's chat history for the thread. Its worktree, branch and the provider's own session files are left on disk.", 13.0, pal.muted);
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Btn::new("Delete thread").kind(w::Kind::Danger).medium().show(ui).clicked() {
                        app.delete_thread(thread);
                        close = true;
                    }
                    if Btn::ghost("Cancel").medium().show(ui).clicked() {
                        close = true;
                    }
                });
            }
            Confirm::RevertHunk { thread, file, hunk } => {
                w::text_semibold(ui, "Revert this hunk?", 16.0, pal.fg);
                ui.add_space(6.0);
                w::text_wrap(ui, "The hunk's lines in the working tree go back to the base version. A checkpoint is saved first, so you can restore them from the timeline.", 13.0, pal.muted);
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Btn::primary("Revert hunk").medium().show(ui).clicked() {
                        app.checkpoint(thread, "Before reverting a hunk".into());
                        app.revert_hunk(thread, *file, *hunk);
                        close = true;
                    }
                    if Btn::ghost("Cancel").medium().show(ui).clicked() {
                        close = true;
                    }
                });
            }
            Confirm::Merge { thread, url } => {
                w::text_semibold(ui, "Merge this pull request?", 16.0, pal.fg);
                ui.add_space(6.0);
                w::text_wrap(ui, format!("Runs `gh pr merge` with “{}” on {url}.", app.settings.merge_method), 13.0, pal.muted);
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Btn::primary("Merge").medium().show(ui).clicked() {
                        app.merge_pr(thread, url.clone());
                        close = true;
                    }
                    if Btn::ghost("Cancel").medium().show(ui).clicked() {
                        close = true;
                    }
                });
            }
            Confirm::CleanupWorktrees { items } => {
                w::text_semibold(ui, format!("Remove {} worktree{}?", items.len(), if items.len() == 1 { "" } else { "s" }), 16.0, pal.fg);
                ui.add_space(6.0);
                w::text_wrap(ui, "Only worktrees without local changes are listed. Branches stay; git refuses to remove anything dirty.", 13.0, pal.muted);
                for (path, _) in items {
                    w::mono_text(ui, path.display().to_string(), 12.0, pal.muted);
                }
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Btn::new("Remove worktrees").kind(w::Kind::Danger).medium().show(ui).clicked() {
                        app.cleanup_worktrees(items.clone());
                        close = true;
                    }
                    if Btn::ghost("Cancel").medium().show(ui).clicked() {
                        close = true;
                    }
                });
            }
        }
    });
    if close || modal.should_close() {
        app.confirm = None;
    }
}

pub fn change_word(k: crate::git::ChangeKind) -> &'static str {
    use crate::git::ChangeKind::*;
    match k {
        Added | Untracked => "added",
        Modified | TypeChanged => "modified",
        Deleted => "deleted",
        Renamed => "renamed",
        Copied => "copied",
        Conflicted => "conflict",
    }
}

/// Command palette (Ctrl+K): actions, threads and projects.
fn palette(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    // Dim everything below the dialog; a click outside it closes the dialog.
    ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::unique("palette_backdrop"))).rect_filled(screen, 0.0, pal.backdrop);
    egui::Area::new(egui::Id::unique("palette_backdrop_hit")).order(egui::Order::Middle).fixed_pos(screen.min).show(&ctx, |ui| {
        ui.set_min_size(screen.size());
        let resp = ui.interact(screen, ui.scope_id().with("hit"), Sense::click());
        if resp.clicked() {
            app.ui.palette_open = false;
        }
    });
    let width = 620.0f32.min(screen.width() - 32.0);
    egui::Area::new(egui::Id::unique("palette"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(screen.center().x - width / 2.0, screen.top() + screen.height() * 0.14))
        .show(&ctx, |ui| {
            w::popover(16).show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    w::icon(ui, ic::SEARCH, 16.0, pal.muted);
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut app.ui.palette_query)
                            .hint_text("Search threads, projects and actions")
                            .frame(Frame::NONE)
                            .font(theme::ui(fs(14.0)))
                            .desired_width(width - 90.0)
                            .margin(vec2(4.0, 14.0)),
                    );
                    r.request_focus();
                    w::kbd(ui, "Esc");
                });
                w::hairline(ui, pal.border);
                let q = app.ui.palette_query.trim().to_lowercase();
                let has = |s: &str| q.is_empty() || s.to_lowercase().contains(&q);
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                type Act = Box<dyn FnOnce(&mut App)>;
                let mut first_action: Option<Act> = None;
                egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                    Frame::new().inner_margin(4).show(ui, |ui| {
                        let cmds: Vec<(&str, &str, &str, Act)> = vec![
                            ("New thread", ic::SQUARE_PEN, "Ctrl N", Box::new(|a: &mut App| a.view = View::NewThread)),
                            ("Open agents board", ic::KANBAN, "", Box::new(|a: &mut App| a.view = View::Board)),
                            (
                                "Review & ship",
                                ic::BADGE_CHECK,
                                "",
                                Box::new(|a: &mut App| {
                                    if a.current.is_some() {
                                        a.view = View::Review
                                    }
                                }),
                            ),
                            (
                                "Turn timeline",
                                ic::HISTORY,
                                "",
                                Box::new(|a: &mut App| {
                                    a.ui.right_open = true;
                                    a.ui.right_tab = Some(RightTab::Timeline);
                                }),
                            ),
                            ("Toggle thread details", ic::SQUARE_MENU, "", Box::new(|a: &mut App| a.ui.details_open = !a.ui.details_open)),
                            (
                                "Toggle terminal drawer",
                                ic::PANEL_BOTTOM,
                                "Ctrl `",
                                Box::new(|a: &mut App| a.ui.terminal_open = !a.ui.terminal_open),
                            ),
                            ("Toggle right panel", ic::PANEL_RIGHT, "", Box::new(|a: &mut App| a.ui.right_open = !a.ui.right_open)),
                            ("Add project", ic::FOLDER_PLUS, "", Box::new(|a: &mut App| a.pick_project_folder())),
                            ("Open settings", ic::SETTINGS, "Ctrl ,", Box::new(|a: &mut App| a.ui.settings_open = true)),
                            (
                                "Worktree settings",
                                ic::FOLDER_TREE,
                                "",
                                Box::new(|a: &mut App| {
                                    a.ui.settings_open = true;
                                    a.ui.settings_page = "worktrees".into();
                                }),
                            ),
                            ("Check provider status", ic::ROTATE_CCW, "", Box::new(|a: &mut App| a.refresh_providers())),
                        ];
                        let mut shown = false;
                        for (label, icon, keys, act) in cmds {
                            if !has(label) {
                                continue;
                            }
                            if !shown {
                                w::menu_heading(ui, "Actions");
                                shown = true;
                            }
                            let hl = first_action.is_none() && !q.is_empty();
                            let (rect, resp) = w::row_button(ui, 32.0, hl, theme::R_MD);
                            w::paint_icon(
                                ui,
                                Rect::from_min_size(egui::pos2(rect.left() + 8.0, rect.center().y - 8.0), vec2(16.0, 16.0)),
                                icon,
                                16.0,
                                pal.muted,
                            );
                            ui.painter().text(
                                egui::pos2(rect.left() + 32.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                label,
                                theme::ui(fs(14.0)),
                                pal.fg,
                            );
                            ui.painter().text(
                                egui::pos2(rect.right() - 8.0, rect.center().y),
                                Align2::RIGHT_CENTER,
                                keys,
                                theme::ui(fs(12.0)),
                                pal.muted,
                            );
                            if resp.clicked() {
                                act(app);
                                app.ui.palette_open = false;
                            } else if first_action.is_none() {
                                first_action = Some(act);
                            }
                        }
                        let threads: Vec<(String, String, String)> = app
                            .threads
                            .iter()
                            .filter(|t| !t.archived)
                            .map(|t| (t.id.clone(), t.title.clone(), app.project_name(t)))
                            .filter(|(_, t, p)| has(&format!("{t} {p}")))
                            .take(30)
                            .collect();
                        if !threads.is_empty() {
                            ui.add_space(4.0);
                            w::menu_heading(ui, "Threads");
                        }
                        for (id, title, proj) in &threads {
                            let (rect, resp) = w::row_button(ui, 32.0, false, theme::R_MD);
                            let (l, fg, bg) = w::project_colors(proj);
                            w::paint_fav(
                                ui,
                                Rect::from_min_size(egui::pos2(rect.left() + 8.0, rect.center().y - 8.0), vec2(16.0, 16.0)),
                                &l,
                                fg,
                                bg,
                            );
                            w::paint_truncated(
                                ui,
                                egui::pos2(rect.left() + 32.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                title,
                                theme::ui(fs(14.0)),
                                pal.fg,
                                rect.width() - 160.0,
                            );
                            ui.painter().text(
                                egui::pos2(rect.right() - 8.0, rect.center().y),
                                Align2::RIGHT_CENTER,
                                proj,
                                theme::ui(fs(12.0)),
                                pal.muted,
                            );
                            if resp.clicked() {
                                app.open_thread(id);
                                app.ui.palette_open = false;
                            }
                            if first_action.is_none() && !q.is_empty() {
                                let id = id.clone();
                                first_action = Some(Box::new(move |a: &mut App| a.open_thread(&id)));
                            }
                        }
                        if !shown && threads.is_empty() {
                            ui.add_space(24.0);
                            ui.vertical_centered(|ui| w::text(ui, "No results.", 12.0, pal.faint));
                            ui.add_space(24.0);
                        }
                    });
                });
                if enter
                    && app.ui.palette_open
                    && let Some(a) = first_action
                {
                    a(app);
                    app.ui.palette_open = false;
                }
            });
        });
}

/// Shared 52px header row; `content` draws the left part.
pub fn header(ui: &mut Ui, right_pad: f32, content: impl FnOnce(&mut Ui)) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(vec2(w, theme::HEADER_H), Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_size(vec2(w, theme::HEADER_H));
        ui.add_space(20.0);
        ui.spacing_mut().item_spacing.x = 12.0;
        let inner = w - 20.0 - right_pad;
        ui.allocate_ui_with_layout(vec2(inner.max(0.0), theme::HEADER_H), Layout::left_to_right(Align::Center), |ui| {
            ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
            content(ui)
        });
    });
}

/// Project favicon + name crumb, then "/".
pub fn crumb_project(ui: &mut Ui, name: &str) -> egui::Response {
    let pal = p();
    let (l, fg, bg) = w::project_colors(name);
    let r = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        w::fav(ui, &l, fg, bg, 14.0);
        w::text(ui, name, 14.0, pal.muted);
    });
    let resp = r.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    w::text(ui, "/", 14.0, pal.muted);
    resp
}

pub fn empty_state(ui: &mut Ui, title: &str, body: &str) {
    ui.add_space(ui.available_height() * 0.3);
    ui.vertical_centered(|ui| {
        w::text_medium(ui, title, 14.0, p().fg);
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new(body).font(theme::ui(fs(13.0))).color(p().muted)).wrap());
    });
}
