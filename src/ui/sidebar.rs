//! Left sidebar: branding, search/filter, thread sections and bottom navigation.

use crate::app::{App, Confirm, View};
use crate::icons as ic;
use crate::model::{RunState, Thread, now};
use crate::theme::{self, fs, mono_fs, p, radius};
use crate::widgets::{self as w};
use egui::{Align2, Color32, Rect, Sense, Ui, pos2, vec2};

#[derive(PartialEq, Clone, Copy)]
enum Sec {
    Top,
    Working,
    Settled,
}

pub fn is_settled(app: &App, t: &Thread) -> bool {
    if t.state.is_active() {
        return false;
    }
    t.settled || (app.settings.auto_settle && t.pr_url.is_none() && now() - t.updated_at > app.settings.auto_settle_days as i64 * 86_400)
}

fn section_of(app: &App, t: &Thread) -> Sec {
    if is_settled(app, t) {
        Sec::Settled
    } else if app.settings.working_section && matches!(t.state, RunState::Working | RunState::Queued) && !t.pinned {
        Sec::Working
    } else {
        Sec::Top
    }
}

/// Threads shown in the sidebar (compare runs collapse into one row), in display order.
fn rows(app: &App) -> Vec<(Sec, Row)> {
    let q = app.ui.sidebar_query.trim().to_lowercase();
    let mut out: Vec<(Sec, Row, bool, i64)> = Vec::new();
    let mut groups_seen = std::collections::HashSet::new();
    for t in app.threads.iter().filter(|t| !t.archived) {
        if let Some(pf) = &app.ui.sidebar_project
            && t.project_id.as_deref() != Some(pf.as_str())
        {
            continue;
        }
        if !q.is_empty()
            && !format!("{} {} {}", t.title, app.project_name(t), t.branch.clone().unwrap_or_default()).to_lowercase().contains(&q)
        {
            continue;
        }
        match (&t.compare_group, t.kept) {
            (Some(g), false) => {
                if groups_seen.insert(g.clone()) {
                    let runs: Vec<&Thread> = app.threads.iter().filter(|x| x.compare_group.as_deref() == Some(g.as_str())).collect();
                    let any_kept = runs.iter().any(|r| r.kept);
                    if any_kept {
                        continue;
                    }
                    let latest = runs.iter().map(|r| r.updated_at).max().unwrap_or(0);
                    out.push((Sec::Top, Row::Compare(g.clone()), false, latest));
                }
            }
            _ => {
                let sec = section_of(app, t);
                out.push((sec, Row::Thread(t.id.clone()), t.pinned, t.updated_at));
            }
        }
    }
    let attention = |r: &Row| match r {
        Row::Thread(id) => app.thread(id).map(|t| t.state == RunState::NeedsApproval).unwrap_or(false),
        Row::Compare(_) => false,
    };
    out.sort_by(|a, b| b.2.cmp(&a.2).then(attention(&b.1).cmp(&attention(&a.1))).then(b.3.cmp(&a.3)));
    out.into_iter().map(|(s, r, _, _)| (s, r)).collect()
}

#[derive(Clone, PartialEq)]
enum Row {
    Thread(String),
    Compare(String),
}

pub fn visible_order(app: &App) -> Vec<String> {
    rows(app)
        .into_iter()
        .filter(|(s, _)| *s != Sec::Settled || !app.ui.collapsed.contains("settled"))
        .filter_map(|(_, r)| match r {
            Row::Thread(id) => Some(id),
            Row::Compare(_) => None,
        })
        .collect()
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let pal = p();
    // Brand row
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 52.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.add_space(12.0);
        ui.spacing_mut().item_spacing.x = 12.0;
        if w::icon_button(ui, ic::PANEL_LEFT, "Toggle main sidebar", false).clicked() {
            app.ui.sidebar_open = false;
        }
        let mut job = egui::text::LayoutJob::default();
        job.append("Kiln", 0.0, egui::TextFormat::simple(theme::ui_semibold(fs(14.0)), pal.fg));
        job.append("Code", 4.0, egui::TextFormat::simple(theme::ui_medium(fs(14.0)), pal.muted));
        ui.add(egui::Label::new(job).selectable(false));
    });

    // Toolbar
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        let wsearch = ui.available_width() - 3.0 * 32.0 - 8.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(wsearch, 32.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, radius(theme::R_MD), pal.hover);
        }
        w::paint_icon(
            ui,
            Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - 8.0), vec2(16.0, 16.0)),
            ic::SEARCH,
            16.0,
            pal.icon,
        );
        ui.painter().text(
            pos2(rect.left() + 32.0, rect.center().y),
            Align2::LEFT_CENTER,
            "Search",
            theme::ui_medium(fs(14.0)),
            pal.section,
        );
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            app.ui.palette_open = true;
            app.ui.palette_query.clear();
        }
        let filter_on = app.ui.sidebar_project.is_some();
        let fr = w::icon_button(ui, ic::LIST_FILTER, "Filter and density", filter_on);
        egui::Popup::menu(&fr).width(ui.available_width().max(260.0)).frame(w::popover(10).inner_margin(4)).show(|ui| {
            ui.set_min_width(250.0);
            w::menu_heading(ui, "Project");
            let mut pick: Option<Option<String>> = None;
            if w::menu_item(ui, app.ui.sidebar_project.is_none().then_some(ic::CHECK), "All projects", None, true).clicked() {
                pick = Some(None);
            }
            for pr in app.projects.iter().filter(|p| !p.archived) {
                let on = app.ui.sidebar_project.as_deref() == Some(pr.id.as_str());
                if w::menu_item(ui, on.then_some(ic::CHECK), &pr.name, None, true).clicked() {
                    pick = Some(Some(pr.id.clone()));
                }
            }
            if let Some(p) = pick {
                app.ui.sidebar_project = p;
                ui.close();
            }
            w::menu_separator(ui);
            w::menu_heading(ui, "Density");
            for d in ["Comfortable", "Compact"] {
                if w::menu_item(ui, (app.settings.sidebar_density == d).then_some(ic::CHECK), d, None, true).clicked() {
                    app.settings.sidebar_density = d.into();
                    ui.close();
                }
            }
        });
        if w::icon_button(ui, ic::FOLDER_PLUS, "Add project", false).clicked() {
            app.pick_project_folder();
        }
        if w::icon_button(ui, ic::SQUARE_PEN, "New thread", app.view == View::NewThread).clicked() {
            app.view = View::NewThread;
            app.ui.focus_composer = true;
        }
    });

    let bottom_h = 44.0;
    let list_h = (ui.available_height() - bottom_h).max(0.0);
    let all = rows(app);
    let compact = app.settings.sidebar_density == "Compact";
    egui::ScrollArea::vertical().id_salt("thread_list").max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin { left: 8, right: 8, top: 0, bottom: 8 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if all.is_empty() {
                ui.add_space(24.0);
                let msg = if app.projects.is_empty() {
                    "Add a project to start a thread."
                } else if !app.ui.sidebar_query.is_empty() || app.ui.sidebar_project.is_some() {
                    "No threads match the filter."
                } else {
                    "No threads yet."
                };
                ui.vertical_centered(|ui| w::text(ui, msg, 12.0, pal.faint));
                if app.projects.is_empty() {
                    ui.add_space(8.0);
                    ui.vertical_centered(|ui| {
                        if w::Btn::new("Add project").icon(ic::FOLDER_PLUS).show(ui).clicked() {
                            app.pick_project_folder();
                        }
                    });
                }
            }
            for sec in [Sec::Top, Sec::Working, Sec::Settled] {
                let items: Vec<Row> = all.iter().filter(|(s, _)| *s == sec).map(|(_, r)| r.clone()).collect();
                if sec != Sec::Top {
                    if items.is_empty() {
                        continue;
                    }
                    let key = if sec == Sec::Working { "working" } else { "settled" };
                    let open = !app.ui.collapsed.contains(key);
                    if section_header(ui, if sec == Sec::Working { "Working" } else { "Settled" }, items.len(), open).clicked() {
                        if open {
                            app.ui.collapsed.insert(key.into());
                        } else {
                            app.ui.collapsed.remove(key);
                        }
                    }
                    if !open {
                        continue;
                    }
                }
                for r in items {
                    match r {
                        Row::Thread(id) => thread_row(app, ui, &id, compact, sec == Sec::Settled),
                        Row::Compare(g) => compare_row(app, ui, &g, compact),
                    }
                }
            }
        });
    });

    // Bottom navigation
    ui.allocate_ui_with_layout(vec2(ui.available_width(), bottom_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        if w::icon_button_sized(ui, ic::SETTINGS, "Settings", app.ui.settings_open, 32.0, 16.0, pal.icon).clicked() {
            app.ui.settings_open = true;
        }
        let prs: Vec<(String, String)> = app.threads.iter().filter_map(|t| t.pr_url.clone().map(|u| (t.title.clone(), u))).collect();
        let pr = w::icon_button_sized(ui, ic::GIT_PR, "Pull requests", false, 32.0, 16.0, pal.icon);
        egui::Popup::menu(&pr).width(320.0).frame(w::popover(10).inner_margin(4)).show(|ui| {
            ui.set_min_width(300.0);
            w::menu_heading(ui, "Pull requests opened from Kiln");
            if prs.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    w::text(ui, "None yet. Open one from Review & ship.", 13.0, pal.muted);
                });
            }
            for (title, url) in prs {
                if w::menu_item(ui, Some(ic::EXTERNAL), &provider_short(&title), None, true).clicked() {
                    let _ = open::that(&url);
                    ui.close();
                }
            }
        });
        if w::icon_button_sized(ui, ic::CHART, "Usage", false, 32.0, 16.0, pal.icon).clicked() {
            app.ui.settings_open = true;
            app.ui.settings_page = "usage".into();
        }
        if w::icon_button_sized(ui, ic::KANBAN, "Agents board", app.view == View::Board, 32.0, 16.0, pal.icon).clicked() {
            app.view =
                if app.view == View::Board { if app.current.is_some() { View::Thread } else { View::NewThread } } else { View::Board };
        }
    });
}

fn provider_short(s: &str) -> String {
    crate::provider::short(s, 40)
}

fn section_header(ui: &mut Ui, label: &str, count: usize, open: bool) -> egui::Response {
    let pal = p();
    ui.add_space(6.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, radius(theme::R_MD), pal.hover);
    }
    let f = theme::ui_medium(fs(12.0));
    let lw = w::galley_width(ui, label, &f);
    ui.painter().text(pos2(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, label, f.clone(), pal.section);
    let cnt = count.to_string();
    let cw = w::galley_width(ui, &cnt, &f);
    let right = rect.right() - 8.0 - 12.0 - 8.0;
    ui.painter().text(pos2(right, rect.center().y), Align2::RIGHT_CENTER, &cnt, f, pal.section);
    ui.painter().hline((rect.left() + 16.0 + lw)..=(right - cw - 8.0), rect.center().y, egui::Stroke::new(1.0, pal.border_faint));
    let chev = Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), vec2(12.0, 12.0));
    w::paint_icon(ui, chev, if open { ic::CHEVRON_DOWN } else { ic::CHEVRON_RIGHT }, 12.0, pal.section);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn status_bits(app: &App, t: &Thread) -> Option<(&'static str, &'static str, Color32, bool)> {
    let pal = p();
    let pending = app.approvals.get(&t.id).is_some_and(|a| !a.is_empty());
    Some(match t.state {
        _ if pending || t.state == RunState::NeedsApproval => ("Approval", ic::SHIELD_QUESTION, pal.warning, false),
        RunState::Working => ("Working", ic::CIRCLE_DASHED, pal.info, true),
        RunState::Queued => ("Queued", ic::CIRCLE_DASHED, pal.muted, false),
        RunState::ReadyToReview => ("Review", ic::CIRCLE_CHECK, pal.success, false),
        RunState::Failed => ("Failed", ic::CIRCLE_X, pal.danger, false),
        RunState::Completed if now() - t.updated_at < 3600 => ("Done", ic::CIRCLE_CHECK, pal.success, false),
        _ => return None,
    })
}

fn thread_row(app: &mut App, ui: &mut Ui, id: &str, compact: bool, settled: bool) {
    let Some(t) = app.thread(id).cloned() else { return };
    let pal = p();
    let selected = app.view == View::Thread && app.current.as_deref() == Some(id);
    let pname = app.project_name(&t);
    let (letter, ffg, fbg) = w::project_colors(&pname);
    let status = status_bits(app, &t);
    let renaming = app.ui.rename.as_ref().filter(|(rid, _)| rid == id).map(|(_, s)| s.clone());
    let slim = compact || settled;
    let h = if slim { 36.0 } else { 78.0 };
    if !slim {
        ui.add_space(1.0);
    }
    let (rect, resp) = w::row_button(ui, h, selected, theme::R_MD);
    let painter = ui.painter().with_clip_rect(rect);
    if slim {
        let fav = Rect::from_min_size(pos2(rect.left() + 10.0, rect.center().y - 8.0), vec2(16.0, 16.0));
        w::paint_fav(ui, fav, &letter, ffg, fbg);
        let right_w = 40.0;
        let fg = if settled { pal.subtle } else { pal.changed_fg };
        if renaming.is_none() {
            w::paint_truncated(
                ui,
                pos2(fav.right() + 10.0, rect.center().y),
                Align2::LEFT_CENTER,
                &t.title,
                theme::ui(fs(14.0)),
                fg,
                rect.width() - 46.0 - right_w,
            );
        }
        match status {
            Some((label, glyph, color, spin)) => {
                let ir = Rect::from_center_size(pos2(rect.right() - 17.0, rect.center().y), vec2(14.0, 14.0));
                if spin {
                    w::paint_spinner(ui, ir, color)
                } else {
                    w::paint_icon(ui, ir, glyph, 14.0, color)
                }
                let _ = label;
            }
            None => {
                painter.text(
                    pos2(rect.right() - 10.0, rect.center().y),
                    Align2::RIGHT_CENTER,
                    w::ago(t.updated_at),
                    theme::ui(fs(12.0)),
                    pal.subtle,
                );
            }
        }
    } else {
        let x0 = rect.left() + 10.0;
        let y1 = rect.top() + 8.0 + 10.0;
        let fav = Rect::from_min_size(pos2(x0, y1 - 8.0), vec2(16.0, 16.0));
        w::paint_fav(ui, fav, &letter, ffg, fbg);
        let mut right = rect.right() - 10.0;
        match status {
            Some((label, glyph, color, spin)) => {
                let f = theme::ui(fs(12.0));
                let lw = w::galley_width(ui, label, &f);
                painter.text(pos2(right, y1), Align2::RIGHT_CENTER, label, f, color);
                let ir = Rect::from_center_size(pos2(right - lw - 4.0 - 7.0, y1), vec2(14.0, 14.0));
                if spin {
                    w::paint_spinner(ui, ir, color)
                } else {
                    w::paint_icon(ui, ir, glyph, 14.0, color)
                }
                right -= lw + 22.0;
            }
            None => {
                let s = w::ago(t.updated_at);
                let f = theme::ui(fs(12.0));
                let sw = w::galley_width(ui, &s, &f);
                painter.text(pos2(right, y1), Align2::RIGHT_CENTER, s, f, pal.subtle);
                right -= sw + 8.0;
            }
        }
        w::paint_truncated(
            ui,
            pos2(fav.right() + 6.0, y1),
            Align2::LEFT_CENTER,
            &pname,
            theme::ui(fs(12.0)),
            pal.subtle,
            right - fav.right() - 10.0,
        );
        if renaming.is_none() {
            w::paint_truncated(
                ui,
                pos2(x0, rect.top() + 8.0 + 20.0 + 4.0),
                Align2::LEFT_TOP,
                &t.title,
                theme::ui_medium(fs(14.0)),
                pal.fg,
                rect.width() - 20.0,
            );
        }
        let y3 = rect.bottom() - 8.0 - 9.0;
        w::paint_icon(ui, Rect::from_center_size(pos2(x0 + 6.0, y3), vec2(12.0, 12.0)), ic::GIT_BRANCH, 12.0, pal.subtle);
        let mut right_meta = rect.right() - 10.0;
        if t.additions + t.deletions > 0 {
            let f = theme::mono(mono_fs(12.0));
            let del = format!("−{}", t.deletions);
            let dw = w::galley_width(ui, &del, &f);
            painter.text(pos2(right_meta, y3), Align2::RIGHT_CENTER, &del, f.clone(), pal.del);
            let add = format!("+{}", t.additions);
            let aw = w::galley_width(ui, &add, &f);
            painter.text(pos2(right_meta - dw - 6.0, y3), Align2::RIGHT_CENTER, &add, f, pal.add);
            right_meta -= dw + aw + 14.0;
        }
        let branch = t
            .branch
            .clone()
            .unwrap_or_else(|| if t.cwd.is_none() && t.is_worktree { "creating worktree…".into() } else { "no branch".into() });
        w::paint_truncated(ui, pos2(x0 + 18.0, y3), Align2::LEFT_CENTER, &branch, theme::ui(fs(12.0)), pal.subtle, right_meta - x0 - 22.0);
    }
    if let Some(mut s) = renaming {
        let r = Rect::from_min_size(
            pos2(rect.left() + 8.0, if slim { rect.top() + 4.0 } else { rect.top() + 26.0 }),
            vec2(rect.width() - 16.0, 26.0),
        );
        let te = ui.put(r, egui::TextEdit::singleline(&mut s).font(theme::ui_medium(fs(14.0))));
        te.request_focus();
        let done = ui.input(|i| i.key_pressed(egui::Key::Enter)) || te.lost_focus();
        let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if cancel {
            app.ui.rename = None;
        } else if done {
            app.ui.rename = None;
            if !s.trim().is_empty() {
                app.rename_thread(id, s.trim().to_string());
            }
        } else {
            app.ui.rename = Some((id.to_string(), s));
        }
    }
    if resp.clicked() {
        app.open_thread(id);
    }
    if resp.double_clicked() {
        app.ui.rename = Some((id.to_string(), t.title.clone()));
    }
    resp.context_menu(|ui| {
        ui.set_min_width(200.0);
        if w::menu_item(ui, Some(ic::SQUARE_PEN), "Rename", None, true).clicked() {
            app.ui.rename = Some((id.to_string(), t.title.clone()));
            ui.close();
        }
        if w::menu_item(ui, Some(ic::PIN), if t.pinned { "Unpin" } else { "Pin to top" }, None, true).clicked() {
            app.toggle_pin(id);
            ui.close();
        }
        if w::menu_item(ui, Some(ic::CHECK), if is_settled(app, &t) { "Move back to inbox" } else { "Settle" }, None, true).clicked() {
            let settled_now = is_settled(app, &t);
            if let Some(x) = app.thread_mut(id) {
                x.settled = !settled_now;
                x.updated_at = now();
            }
            if let Some(x) = app.thread(id) {
                app.store.save_thread(x);
            }
            ui.close();
        }
        if let Some(cwd) = app.cwd_of(&t)
            && w::menu_item(ui, Some(ic::FOLDER), "Open folder", None, true).clicked()
        {
            let _ = open::that(cwd);
            ui.close();
        }
        w::menu_separator(ui);
        if w::menu_item(ui, Some(ic::ARCHIVE), "Archive", None, true).clicked() {
            app.set_archived(id, true);
            ui.close();
        }
        if w::menu_item(ui, Some(ic::TRASH), "Delete…", None, true).clicked() {
            if app.settings.delete_confirm {
                app.confirm = Some(Confirm::DeleteThread { thread: id.to_string() });
            } else {
                app.delete_thread(id);
            }
            ui.close();
        }
    });
}

fn compare_row(app: &mut App, ui: &mut Ui, group: &str, compact: bool) {
    let pal = p();
    let runs: Vec<Thread> = app.threads.iter().filter(|t| t.compare_group.as_deref() == Some(group)).cloned().collect();
    let Some(first) = runs.first() else { return };
    let selected = app.view == View::Compare(group.to_string());
    let pname = app.project_name(first);
    let (letter, ffg, fbg) = w::project_colors(&pname);
    let done = runs.iter().filter(|r| !r.state.is_active()).count();
    let (rect, resp) = w::row_button(ui, if compact { 36.0 } else { 78.0 }, selected, theme::R_MD);
    let painter = ui.painter().with_clip_rect(rect);
    let x0 = rect.left() + 10.0;
    if compact {
        w::paint_fav(ui, Rect::from_min_size(pos2(x0, rect.center().y - 8.0), vec2(16.0, 16.0)), &letter, ffg, fbg);
        w::paint_truncated(
            ui,
            pos2(x0 + 26.0, rect.center().y),
            Align2::LEFT_CENTER,
            &format!("Compare · {}", first.title),
            theme::ui(fs(14.0)),
            pal.changed_fg,
            rect.width() - 70.0,
        );
        w::paint_icon(
            ui,
            Rect::from_center_size(pos2(rect.right() - 17.0, rect.center().y), vec2(14.0, 14.0)),
            ic::GIT_COMPARE,
            14.0,
            pal.info,
        );
    } else {
        let y1 = rect.top() + 18.0;
        w::paint_fav(ui, Rect::from_min_size(pos2(x0, y1 - 8.0), vec2(16.0, 16.0)), &letter, ffg, fbg);
        painter.text(pos2(x0 + 22.0, y1), Align2::LEFT_CENTER, &pname, theme::ui(fs(12.0)), pal.subtle);
        let label = if done == runs.len() { "Compared" } else { "Comparing" };
        let f = theme::ui(fs(12.0));
        let lw = w::galley_width(ui, label, &f);
        painter.text(pos2(rect.right() - 10.0, y1), Align2::RIGHT_CENTER, label, f, pal.info);
        w::paint_icon(
            ui,
            Rect::from_center_size(pos2(rect.right() - 10.0 - lw - 11.0, y1), vec2(14.0, 14.0)),
            ic::GIT_COMPARE,
            14.0,
            pal.info,
        );
        w::paint_truncated(
            ui,
            pos2(x0, rect.top() + 32.0),
            Align2::LEFT_TOP,
            &format!("Compare · {}", first.title),
            theme::ui_medium(fs(14.0)),
            pal.fg,
            rect.width() - 20.0,
        );
        let y3 = rect.bottom() - 17.0;
        w::paint_icon(ui, Rect::from_center_size(pos2(x0 + 6.0, y3), vec2(12.0, 12.0)), ic::GIT_BRANCH, 12.0, pal.subtle);
        painter.text(
            pos2(x0 + 18.0, y3),
            Align2::LEFT_CENTER,
            format!("{} runs in separate worktrees", runs.len()),
            theme::ui(fs(12.0)),
            pal.subtle,
        );
    }
    if resp.clicked() {
        app.view = View::Compare(group.to_string());
    }
}
