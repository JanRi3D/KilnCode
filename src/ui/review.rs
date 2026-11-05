//! Review & ship: file navigation with viewed tracking, line comments sent to the agent,
//! local checks, commit message, push and pull-request creation through the gh CLI.

use super::diff;
use crate::app::{App, Confirm, View};
use crate::git::GhAuth;
use crate::icons as ic;
use crate::model::ItemKind;
use crate::theme::{self, fs, mono_fs, p, radius};
use crate::widgets::{self as w, Btn};
use egui::{Align, Frame, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};

pub fn show(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let Some(id) = app.current.clone().filter(|id| app.thread(id).is_some()) else {
        app.view = View::NewThread;
        return;
    };
    let t = app.thread(&id).cloned().expect("thread");
    if app.git.get(&id).is_none_or(|g| g.diff.is_none() && !g.diff_loading) {
        app.load_diff(&id);
    }
    let pname = app.project_name(&t);
    super::header(ui, 20.0, |ui| {
        if !app.ui.sidebar_open && w::icon_button(ui, ic::PANEL_LEFT, "Show sidebar", false).clicked() {
            app.ui.sidebar_open = true;
        }
        super::crumb_project(ui, &pname);
        let r = ui.add(
            egui::Label::new(egui::RichText::new(format!("Review & ship · {}", t.title)).font(theme::ui_medium(fs(14.0))).color(pal.fg))
                .truncate()
                .sense(Sense::click()),
        );
        if r.on_hover_text("Back to the thread").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            app.view = View::Thread;
        }
    });
    let area = ui.available_rect_before_wrap();
    ui.painter().hline(area.x_range(), area.top(), Stroke::new(1.0, pal.border_soft));
    let narrow = area.width() < 980.0;
    let left_w = if narrow { 220.0 } else { 260.0 };
    let right_w = if narrow { 280.0 } else { 340.0 };
    let left = Rect::from_min_size(area.min, vec2(left_w, area.height()));
    let right = Rect::from_min_max(pos2(area.right() - right_w, area.top()), area.max);
    let center = Rect::from_min_max(pos2(left.right(), area.top()), pos2(right.left(), area.bottom()));
    ui.painter().vline(left.right(), area.y_range(), Stroke::new(1.0, pal.border));
    ui.painter().vline(right.left(), area.y_range(), Stroke::new(1.0, pal.border));
    let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(left.shrink2(vec2(8.0, 10.0))));
    files_panel(app, &mut lui, &id);
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(center));
    diff_panel(app, &mut cui, &id);
    let mut rui = ui.new_child(egui::UiBuilder::new().max_rect(right.shrink(16.0)));
    pr_panel(app, &mut rui, &id);
    ui.allocate_rect(area, Sense::hover());
}

fn files_panel(app: &mut App, ui: &mut Ui, id: &str) {
    let pal = p();
    let doc = app.git.get(id).and_then(|g| g.diff.clone());
    let viewed = app.viewed.get(id).cloned().unwrap_or_default();
    egui::ScrollArea::vertical().id_salt("review_files").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let (n, v) = doc
            .as_ref()
            .map(|d| (d.files.len(), d.files.iter().zip(&d.fingerprints).filter(|(f, fp)| viewed.get(&f.path) == Some(fp)).count()))
            .unwrap_or((0, 0));
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            w::text_medium(ui, "Files", 12.0, pal.muted);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(8.0);
                w::text_medium(ui, format!("{v} of {n} viewed"), 12.0, pal.muted);
            });
        });
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            w::progress(ui, ui.available_width() - 8.0, if n == 0 { 0.0 } else { v as f32 / n as f32 }, pal.success);
        });
        ui.add_space(4.0);
        match &doc {
            None => {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    w::text(ui, "Loading…", 12.0, pal.faint);
                });
            }
            Some(d) if d.files.is_empty() => {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    w::text(ui, "No changes to review.", 12.0, pal.faint);
                });
            }
            Some(d) => {
                let comments = app.comments.get(id).cloned().unwrap_or_default();
                for (i, f) in d.files.iter().enumerate() {
                    let on = i == app.ui.review_file.min(d.files.len() - 1);
                    let is_v = viewed.get(&f.path) == d.fingerprints.get(i);
                    let nc = comments.iter().filter(|c| c.path == f.path && !c.resolved).count();
                    Frame::new().fill(if on { pal.hover } else { egui::Color32::TRANSPARENT }).corner_radius(radius(theme::R_LG)).show(
                        ui,
                        |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                if diff::paint_checkbox(ui, is_v).on_hover_text(format!("Mark {} as viewed", f.path)).clicked() {
                                    app.set_viewed(id, &f.path, (!is_v).then(|| d.fingerprints[i].clone()));
                                }
                                let r = ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.add_space(6.0);
                                    let name = f.path.rsplit('/').next().unwrap_or(&f.path);
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(name).font(theme::mono(mono_fs(12.0))).color(pal.fg))
                                            .truncate(),
                                    )
                                    .on_hover_text(&f.path);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        if f.binary {
                                            w::text(ui, "binary", 11.0, pal.muted);
                                        } else {
                                            w::diff_counts(ui, f.additions, f.deletions, 11.0);
                                        }
                                        if nc > 0 {
                                            w::icon(ui, ic::MSG_PLUS, 11.0, pal.muted);
                                            w::text(ui, nc.to_string(), 11.0, pal.muted);
                                        }
                                    });
                                    ui.add_space(6.0);
                                });
                                if r.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    app.ui.review_file = i;
                                    app.ui.comment_line = None;
                                }
                            });
                        },
                    );
                }
            }
        }
        ui.add_space(18.0);
        checks(app, ui, id);
    });
}

fn check_row(ui: &mut Ui, glyph: &str, color: egui::Color32, label: &str, value: &str) {
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 28.0), Layout::left_to_right(Align::Center), |ui| {
        ui.add_space(8.0);
        w::icon(ui, glyph, 14.0, color);
        w::text(ui, label, 13.0, p().fg);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(8.0);
            w::text(ui, value, 12.0, color);
        });
    });
}

fn checks(app: &mut App, ui: &mut Ui, id: &str) {
    let pal = p();
    let Some(t) = app.thread(id).cloned() else { return };
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        w::text_medium(ui, "Checks", 12.0, pal.muted);
    });
    let testing = app.ui.busy.contains(&format!("tests:{id}"));
    let (g, c, v) = match (testing, t.test_ok) {
        (true, _) => (ic::CIRCLE_DASHED, pal.info, "Running…".to_string()),
        (_, Some(true)) => (ic::CIRCLE_CHECK, pal.success, t.test_summary.clone().unwrap_or_else(|| "Passed".into())),
        (_, Some(false)) => (ic::CIRCLE_X, pal.danger, t.test_summary.clone().unwrap_or_else(|| "Failed".into())),
        _ => (ic::CIRCLE, pal.muted, "Not run".into()),
    };
    check_row(ui, g, c, "Tests (local)", &v);
    let (files, _, _) = app.git.get(id).map(|g| g.totals()).unwrap_or((0, 0, 0));
    if files > 0 {
        check_row(
            ui,
            ic::CIRCLE_DASHED,
            pal.warning,
            "Working tree",
            &format!("{files} file{} changed", if files == 1 { "" } else { "s" }),
        );
    } else {
        check_row(ui, ic::CIRCLE_CHECK, pal.success, "Working tree", "No changes");
    }
    match &t.pr_url {
        Some(_) => check_row(ui, ic::GLOBE, pal.info, "Remote CI", "On GitHub"),
        None => check_row(ui, ic::CIRCLE, pal.muted, "Remote CI", "After you open a PR"),
    }
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if Btn::new(if testing { "Running tests…" } else { "Run tests" }).icon(ic::TEST_TUBE).enabled(!testing).show(ui).clicked() {
            app.run_tests(id);
        }
        if let Some(u) = &t.pr_url
            && Btn::ghost("View checks").icon(ic::EXTERNAL).show(ui).clicked()
        {
            let _ = open::that(format!("{u}/checks"));
        }
    });
}

fn diff_panel(app: &mut App, ui: &mut Ui, id: &str) {
    let pal = p();
    let Some(doc) = app.git.get(id).and_then(|g| g.diff.clone()) else {
        ui.centered_and_justified(|ui| {
            w::text(ui, app.git.get(id).and_then(|g| g.error.clone()).unwrap_or_else(|| "Loading the diff…".into()), 12.0, pal.faint)
        });
        return;
    };
    if doc.files.is_empty() {
        ui.centered_and_justified(|ui| w::text(ui, "Nothing changed in this thread's workspace.", 12.0, pal.faint));
        return;
    }
    let fi = app.ui.review_file.min(doc.files.len() - 1);
    let f = &doc.files[fi];
    let viewed = app.viewed.get(id).cloned().unwrap_or_default();
    let is_v = viewed.get(&f.path) == doc.fingerprints.get(fi);
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().rect_filled(bar, 0.0, pal.bg);
    ui.painter().hline(bar.x_range(), bar.bottom(), Stroke::new(1.0, pal.border_soft));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(16.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
    let mut job = w::path_job(&f.path, 12.0, &pal);
    job.wrap.max_width = (bar.width() - 420.0).max(120.0);
    job.wrap.max_rows = 1;
    bui.add(egui::Label::new(job));
    w::diff_counts(&mut bui, f.additions, f.deletions, 12.0);
    bui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let r = ui.horizontal(|ui| {
            w::text(ui, "Viewed", 12.0, pal.muted);
            diff::paint_checkbox(ui, is_v)
        });
        if r.inner.clicked() {
            app.set_viewed(id, &f.path, (!is_v).then(|| doc.fingerprints[fi].clone()));
        }
        let split = app.ui.diff_split.unwrap_or(app.settings.diff_layout == "Split");
        if w::icon_button_sized(
            ui,
            if split { ui_rows() } else { ic::COLUMNS2 },
            if split { "Stacked" } else { "Split" },
            false,
            24.0,
            14.0,
            pal.muted,
        )
        .clicked()
        {
            app.ui.diff_split = Some(!split);
        }
        w::text(ui, "Click a line number to comment", 12.0, pal.muted);
    });
    let split = app.ui.diff_split.unwrap_or(app.settings.diff_layout == "Split");
    let opts = diff::Opts {
        split,
        wrap: app.ui.diff_wrap.unwrap_or(app.settings.word_wrap),
        only_file: Some(fi),
        file_headers: false,
        revert: true,
        comments: true,
        hide_ws: app.settings.hide_whitespace,
    };
    let comments = app.comments.get(id).cloned().unwrap_or_default();
    let composer = app.ui.comment_line;
    let mut draft = std::mem::take(&mut app.ui.comment_draft);
    let acts = diff::show(ui, &format!("review{id}{fi}"), &doc, &opts, &Default::default(), None, &comments, composer, &mut draft);
    app.ui.comment_draft = draft;
    for a in acts {
        match a {
            diff::Action::StartComment(f, l, old) => {
                app.ui.comment_line = Some((f, l, old));
                app.ui.comment_draft.clear();
            }
            diff::Action::CancelComment => app.ui.comment_line = None,
            diff::Action::SubmitComment(f, l, old) => {
                let text = std::mem::take(&mut app.ui.comment_draft);
                let path = doc.files[f].path.clone();
                let rev = doc.fingerprints[f].clone();
                app.add_comment(id, &path, l, old, rev, text.trim().to_string());
                app.ui.comment_line = None;
            }
            diff::Action::SendComment(cid) => app.send_comment(id, &cid),
            diff::Action::ResolveComment(cid) => app.resolve_comment(id, &cid),
            diff::Action::Revert(f, h) => app.confirm = Some(Confirm::RevertHunk { thread: id.to_string(), file: f, hunk: h }),
            _ => {}
        }
    }
}

fn ui_rows() -> &'static str {
    ic::ROWS3
}

fn pr_panel(app: &mut App, ui: &mut Ui, id: &str) {
    let pal = p();
    let Some(t) = app.thread(id).cloned() else { return };
    let busy_pr = app.ui.busy.contains(&format!("pr:{id}"));
    let busy_commit = app.ui.busy.contains(&format!("commit:{id}"));
    egui::ScrollArea::vertical().id_salt("pr_panel").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 14.0;
        w::text_semibold(ui, "Pull request", 16.0, pal.fg);
        if let Some(url) = &t.pr_url {
            Frame::new().fill(theme::tint(pal.success, 0.06)).stroke(Stroke::new(1.0, theme::tint(pal.success, 0.28))).corner_radius(radius(theme::R_XL)).inner_margin(12).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    w::icon(ui, ic::GIT_PR, 16.0, pal.success);
                    w::text_medium(ui, "Pull request opened", 14.0, pal.fg);
                });
                ui.add(egui::Label::new(egui::RichText::new(url).font(theme::mono(mono_fs(12.0))).color(pal.muted)).wrap());
                if Btn::new("Open on GitHub").icon(ic::EXTERNAL).medium().show(ui).clicked() {
                    let _ = open::that(url);
                }
            });
            ui.add(egui::Label::new(egui::RichText::new("Kiln doesn't watch remote CI; open the checks on GitHub.").font(theme::ui(fs(12.0))).color(pal.muted)).wrap());
            if t.state.is_active() {
                return;
            }
            if Btn::new("Merge…").icon(ic::GIT_PR).medium().show(ui).on_hover_text(format!("gh pr merge with “{}”", app.settings.merge_method)).clicked() {
                app.confirm = Some(Confirm::Merge { thread: id.to_string(), url: url.clone() });
            }
            return;
        }
        let final_msg = app.items.get(id).and_then(|l| l.iter().rev().find_map(|i| if let ItemKind::Assistant { text } = &i.kind { (!text.trim().is_empty()).then(|| text.clone()) } else { None }));
        let files: Vec<String> = app.git.get(id).map(|g| g.changes.iter().map(|c| c.path.clone()).collect()).unwrap_or_default();
        let title = app.ui.pr_title.entry(id.to_string()).or_insert_with(|| t.title.clone());
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            w::text_medium(ui, "Title", 12.0, pal.muted);
            w::text_input(ui, title, "Pull request title", ui.available_width(), false);
        });
        let body = app.ui.pr_body.entry(id.to_string()).or_insert_with(|| {
            let mut b = String::from("## Summary\n");
            if let Some(m) = &final_msg {
                b.push_str(m.split("\n\n").next().unwrap_or(m).trim());
                b.push('\n');
            }
            if !files.is_empty() {
                b.push_str("\n## Changes\n");
                for f in files.iter().take(20) {
                    b.push_str(&format!("- `{f}`\n"));
                }
            }
            if let Some(ts) = &t.test_summary {
                b.push_str(&format!("\n## Test plan\n- Local: {ts}\n"));
            }
            b
        });
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.horizontal(|ui| {
                w::text_medium(ui, "Description", 12.0, pal.muted);
                w::icon(ui, ic::SPARKLES, 11.0, pal.muted);
                w::text(ui, "from the thread", 12.0, pal.muted);
            });
            Frame::new().fill(pal.ctrl_bg).stroke(Stroke::new(1.0, pal.input_border)).corner_radius(radius(theme::R_LG)).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(body).frame(Frame::NONE).font(theme::mono(mono_fs(12.0))).desired_rows(9).desired_width(f32::INFINITY));
            });
        });
        let commit = app.thread(id).and_then(|t| t.commit_message.clone()).unwrap_or_else(|| t.title.clone());
        let mut commit_edit = commit.clone();
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            w::text_medium(ui, "Commit message", 12.0, pal.muted);
            Frame::new().fill(pal.ctrl_bg).stroke(Stroke::new(1.0, pal.input_border)).corner_radius(radius(theme::R_LG)).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut commit_edit).frame(Frame::NONE).font(theme::mono(mono_fs(12.0))).desired_rows(2).desired_width(f32::INFINITY));
            });
        });
        if commit_edit != commit {
            if let Some(th) = app.thread_mut(id) {
                th.commit_message = Some(commit_edit.clone());
            }
            if let Some(th) = app.thread(id) {
                app.store.save_thread(th);
            }
        }
        let project = t.project_id.clone().unwrap_or_default();
        let (branches, def) = app.branches.get(&project).cloned().unwrap_or_default();
        let base_default = t.base_branch.clone().filter(|b| branches.contains(b)).unwrap_or(def.clone());
        let mut base = app.ui.pr_base.get(id).cloned().filter(|b| !b.is_empty()).unwrap_or(base_default);
        let mut draft_on = *app.ui.pr_draft.get(id).unwrap_or(&app.settings.pr_draft_default);
        w::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), Layout::left_to_right(Align::Center), |ui| {
                ui.add_space(12.0);
                w::text(ui, "Base branch", 13.0, pal.fg);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(8.0);
                    let opts: Vec<&str> = branches.iter().filter(|b| !b.contains('/') || b.starts_with("origin/")).map(|s| s.as_str()).take(40).collect();
                    w::select(ui, ("pr_base", id), &mut base, &opts, 150.0);
                });
            });
            w::hairline(ui, pal.border_faint);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), Layout::left_to_right(Align::Center), |ui| {
                ui.add_space(12.0);
                w::text(ui, if draft_on { "Draft" } else { "Ready for review" }, 13.0, pal.fg);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(4.0);
                    w::switch(ui, &mut draft_on).on_hover_text("Open as draft");
                });
            });
            w::hairline(ui, pal.border_faint);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), Layout::left_to_right(Align::Center), |ui| {
                ui.add_space(12.0);
                w::text(ui, "Head branch", 13.0, pal.fg);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(12.0);
                    w::mono_text(ui, t.branch.clone().unwrap_or_else(|| "none".into()), 12.0, pal.muted);
                });
            });
        });
        if !base.is_empty() {
            app.ui.pr_base.insert(id.to_string(), base.clone());
        }
        app.ui.pr_draft.insert(id.to_string(), draft_on);
        let all_viewed = app.all_viewed(id);
        if app.settings.require_viewed && !all_viewed {
            w::row_top(ui, |ui| {
                w::icon(ui, ic::SHIELD_ALERT, 14.0, pal.warning);
                ui.add(egui::Label::new(egui::RichText::new("Mark every file as viewed to commit or open the pull request. Your Diffs settings require a look at every file.").font(theme::ui(fs(12.0))).color(pal.warning)).wrap());
            });
        }
        let gh_ok = matches!(app.gh.as_ref().map(|g| &g.0), Some(GhAuth::SignedIn { .. }));
        if !gh_ok {
            let msg = match app.gh.as_ref().map(|g| &g.0) {
                Some(GhAuth::NotInstalled) => "Install the GitHub CLI (gh) to open pull requests from Kiln.".to_string(),
                Some(GhAuth::SignedOut(_)) => "GitHub CLI is signed out. Run `gh auth login` in a terminal to open pull requests.".to_string(),
                _ => "Checking GitHub CLI…".to_string(),
            };
            ui.add(egui::Label::new(egui::RichText::new(msg).font(theme::ui(fs(12.0))).color(pal.muted)).wrap());
        }
        let gated = app.settings.require_viewed && !all_viewed;
        let (files_n, _, _) = app.git.get(id).map(|g| g.totals()).unwrap_or((0, 0, 0));
        ui.horizontal(|ui| {
            let wfull = ui.available_width();
            if Btn::new(if busy_commit { "Working…" } else { "Commit & push" }).large().min_width(wfull * 0.42).enabled(!busy_commit && !gated && t.branch.is_some() && (files_n > 0 || t.branch.is_some())).show(ui).clicked() {
                app.commit_and_push(id, commit_edit.clone(), true);
            }
            let title = app.ui.pr_title.get(id).cloned().unwrap_or_default();
            let body = app.ui.pr_body.get(id).cloned().unwrap_or_default();
            let ok = !busy_pr && !gated && gh_ok && !title.trim().is_empty() && t.branch.is_some();
            if Btn::primary(if busy_pr { "Opening…" } else { "Open pull request" }).large().min_width(ui.available_width()).enabled(ok).show(ui).clicked() {
                if files_n > 0 {
                    app.toast("Commit your changes first (Commit & push), then open the pull request.", false);
                } else {
                    app.open_pr(id, title.trim().to_string(), body, base.clone(), draft_on);
                }
            }
        });
    });
}
