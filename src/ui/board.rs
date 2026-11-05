//! Agents board: every thread by what it needs, filtered by project.

use crate::app::{App, Confirm, View};
use crate::icons as ic;
use crate::model::{RunState, StepStatus, Thread, now};
use crate::provider::Decision;
use crate::theme::{self, fs, mono_fs, p, radius};
use crate::widgets::{self as w, Btn};
use egui::{Align, Frame, Layout, Margin, Sense, Stroke, Ui, vec2};

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Need,
    Run,
    Review,
    Done,
}

fn column(app: &App, t: &Thread) -> Col {
    if app.approvals.get(&t.id).is_some_and(|a| !a.is_empty()) || matches!(t.state, RunState::NeedsApproval | RunState::Failed) {
        Col::Need
    } else if matches!(t.state, RunState::Working | RunState::Queued) {
        Col::Run
    } else if super::sidebar::is_settled(app, t) || t.files_changed == 0 && t.pr_url.is_none() {
        Col::Done
    } else {
        Col::Review
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let threads: Vec<Thread> = app.threads.iter().filter(|t| !t.archived).cloned().collect();
    let counts = |c: Col| threads.iter().filter(|t| column(app, t) == c).count();
    let summary = format!("{} need you · {} working · {} to review", counts(Col::Need), counts(Col::Run), counts(Col::Review));
    let mut projects: Vec<(Option<String>, String, usize)> = vec![(None, "All".into(), threads.len())];
    for pr in app.projects.iter().filter(|p| !p.archived) {
        let n = threads.iter().filter(|t| t.project_id.as_deref() == Some(pr.id.as_str())).count();
        projects.push((Some(pr.id.clone()), pr.name.clone(), n));
    }
    super::header(ui, 20.0, |ui| {
        if !app.ui.sidebar_open && w::icon_button(ui, ic::PANEL_LEFT, "Show sidebar", false).clicked() {
            app.ui.sidebar_open = true;
        }
        w::text_medium(ui, "Agents board", 14.0, pal.fg);
        w::text(ui, &summary, 14.0, pal.muted);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (id, name, n) in projects.iter().rev() {
                let on = app.ui.board_filter == *id;
                let label = format!("{name} {n}");
                if Btn::new(&label).medium().font(13.0).selected(on).show(ui).clicked() {
                    app.ui.board_filter = id.clone();
                }
            }
        });
    });
    let filtered: Vec<Thread> =
        threads.into_iter().filter(|t| app.ui.board_filter.is_none() || t.project_id == app.ui.board_filter).collect();
    egui::ScrollArea::vertical().id_salt("board").auto_shrink([false, false]).show(ui, |ui| {
        Frame::new().inner_margin(Margin { left: 20, right: 20, top: 8, bottom: 32 }).show(ui, |ui| {
            let avail = ui.available_width();
            let ncols = ((avail + 16.0) / 276.0).floor().clamp(1.0, 4.0) as usize;
            let cw = (avail - 16.0 * (ncols as f32 - 1.0)) / ncols as f32;
            let cols = [
                (Col::Need, "Needs you", ic::SHIELD_QUESTION, pal.warning, "Nothing is waiting on you."),
                (Col::Run, "Working", ic::CIRCLE_DASHED, pal.info, "No agents running."),
                (Col::Review, "Ready to review", ic::CIRCLE_CHECK, pal.success, "Nothing to review."),
                (Col::Done, "Settled", ic::CHECK, pal.muted, "Nothing settled yet."),
            ];
            for chunk in cols.chunks(ncols) {
                w::row_top(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    for (col, label, glyph, color, empty) in chunk {
                        ui.vertical(|ui| {
                            ui.set_width(cw);
                            ui.spacing_mut().item_spacing.y = 8.0;
                            let cards: Vec<&Thread> = filtered.iter().filter(|t| column(app, t) == *col).collect();
                            ui.allocate_ui_with_layout(vec2(cw, 32.0), Layout::left_to_right(Align::Center), |ui| {
                                ui.add_space(8.0);
                                w::icon(ui, glyph, 14.0, *color);
                                w::text_medium(ui, *label, 12.0, pal.section);
                                let (r, _) = ui.allocate_exact_size(vec2((ui.available_width() - 30.0).max(0.0), 1.0), Sense::hover());
                                ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, pal.border_faint));
                                w::text_medium(ui, cards.len().to_string(), 12.0, pal.section);
                            });
                            if cards.is_empty() {
                                ui.add_space(16.0);
                                ui.vertical_centered(|ui| w::text(ui, *empty, 12.0, pal.faint));
                                ui.add_space(16.0);
                            }
                            for t in cards {
                                card(app, ui, t, *col);
                            }
                        });
                    }
                });
                ui.add_space(16.0);
            }
        });
    });
}

fn card(app: &mut App, ui: &mut Ui, t: &Thread, col: Col) {
    let pal = p();
    let pname = app.project_name(t);
    let (l, fg, bg) = w::project_colors(&pname);
    w::card().inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            w::fav(ui, &l, fg, bg, 16.0);
            w::text(ui, &pname, 12.0, pal.subtle);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let time = match t.turn_started_at {
                    Some(s) if t.state == RunState::Working => w::duration(now() - s),
                    _ => w::ago(t.updated_at),
                };
                w::text(ui, time, 12.0, pal.subtle);
            });
        });
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let title = match &t.run_label {
                Some(r) => format!("Run {r} · {}", t.title),
                None => t.title.clone(),
            };
            let r = ui.add(
                egui::Label::new(egui::RichText::new(title).font(theme::ui_medium(fs(14.0))).color(pal.fg)).wrap().sense(Sense::click()),
            );
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                app.open_thread(&t.id);
            }
            let detail = match col {
                Col::Need if t.state == RunState::Failed => {
                    t.last_error.clone().map(|e| crate::provider::short(&e, 90)).unwrap_or_else(|| "Failed".into())
                }
                _ => t.last_activity.clone().unwrap_or_else(|| t.state.label().into()),
            };
            w::text(ui, detail, 12.0, if t.state == RunState::Failed { pal.danger } else { pal.muted });
        });
        if col == Col::Run && !t.plan.is_empty() {
            let done = t.plan.iter().filter(|s| s.status == StepStatus::Completed).count();
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                w::progress(ui, ui.available_width(), done as f32 / t.plan.len() as f32, pal.info);
                w::text(ui, format!("Step {} of {} (from the agent's plan)", (done + 1).min(t.plan.len()), t.plan.len()), 11.0, pal.muted);
            });
        }
        if col == Col::Need
            && let Some(a) = app.approvals.get(&t.id).and_then(|l| l.first()).cloned()
        {
            Frame::new()
                .fill(theme::tint(pal.warning, 0.08))
                .stroke(Stroke::new(1.0, theme::tint(pal.warning, 0.32)))
                .corner_radius(radius(theme::R_LG))
                .inner_margin(Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 8.0;
                    w::text(ui, &a.title, 12.0, pal.warning);
                    if let Some(c) = &a.command {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(crate::provider::short(c, 200)).font(theme::mono(mono_fs(11.5))).color(pal.fg),
                            )
                            .wrap(),
                        );
                    }
                    ui.horizontal(|ui| {
                        if Btn::new("Decline").show(ui).clicked() {
                            app.decide(&t.id, &a.request_id, Decision::Deny { message: None }, None);
                        }
                        if Btn::primary("Approve").show(ui).clicked() {
                            app.decide(&t.id, &a.request_id, Decision::Allow, None);
                        }
                        if Btn::ghost("Open").show(ui).clicked() {
                            app.open_thread(&t.id);
                        }
                    });
                });
        }
        if col == Col::Review {
            ui.horizontal(|ui| {
                if Btn::new("Review diff").show(ui).clicked() {
                    app.open_thread(&t.id);
                    app.view = View::Review;
                }
                match &t.pr_url {
                    Some(u) => {
                        if Btn::primary("Merge…").show(ui).clicked() {
                            app.confirm = Some(Confirm::Merge { thread: t.id.clone(), url: u.clone() });
                        }
                    }
                    None => {
                        if Btn::primary("Review & ship").show(ui).clicked() {
                            app.open_thread(&t.id);
                            app.view = View::Review;
                        }
                    }
                }
            });
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            w::icon(ui, ic::GIT_BRANCH, 12.0, pal.subtle);
            let bw = ui.available_width() - if t.additions + t.deletions > 0 { 80.0 } else { 0.0 };
            ui.allocate_ui(vec2(bw, 16.0), |ui| {
                w::text(ui, t.branch.clone().unwrap_or_else(|| "no branch".into()), 12.0, pal.subtle);
            });
            if t.additions + t.deletions > 0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    w::diff_counts(ui, t.additions, t.deletions, 12.0);
                });
            }
        });
    });
}
