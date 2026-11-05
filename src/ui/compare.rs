//! Compare runs: one prompt, several agents, separate worktrees from one base commit.

use crate::app::App;
use crate::icons as ic;
use crate::model::{ItemKind, ProviderKind, RunState, Thread, now};
use crate::theme::{self, fs, mono_fs, p, radius};
use crate::widgets::{self as w, Btn};
use egui::{Align, Frame, Layout, Margin, Sense, Stroke, Ui, vec2};
use std::collections::BTreeMap;

pub fn show(app: &mut App, ui: &mut Ui, group: &str) {
    let pal = p();
    let runs: Vec<Thread> = app.threads.iter().filter(|t| t.compare_group.as_deref() == Some(group)).cloned().collect();
    if runs.is_empty() {
        super::empty_state(ui, "This comparison no longer exists.", "Its runs were deleted.");
        return;
    }
    for r in &runs {
        if !app.git.contains_key(&r.id) && r.cwd.is_some() {
            app.refresh_git(&r.id);
        }
    }
    let done = runs.iter().filter(|r| !r.state.is_active()).count();
    let right_pad = 20.0;
    let pname = app.project_name(&runs[0]);
    super::header(ui, right_pad, |ui| {
        if !app.ui.sidebar_open && w::icon_button(ui, ic::PANEL_LEFT, "Show sidebar", false).clicked() {
            app.ui.sidebar_open = true;
        }
        super::crumb_project(ui, &pname);
        w::text_medium(ui, "Compare runs", 14.0, pal.fg);
        w::pill(ui, Some(ic::GIT_COMPARE), &format!("{} runs · {done} done", runs.len()), pal.info, 22.0, false);
    });
    let prompt = app
        .items
        .get(&runs[0].id)
        .and_then(|l| l.iter().find_map(|i| if let ItemKind::User { text, .. } = &i.kind { Some(text.clone()) } else { None }))
        .unwrap_or_else(|| {
            let items = app.store.load_items(&runs[0].id);
            items.iter().find_map(|i| if let ItemKind::User { text, .. } = &i.kind { Some(text.clone()) } else { None }).unwrap_or_default()
        });
    let base = runs[0].base_rev.clone().map(|b| b[..b.len().min(7)].to_string()).unwrap_or_else(|| "…".into());
    let base_name = runs[0].base_branch.clone().unwrap_or_else(|| "the base branch".into());
    egui::ScrollArea::vertical().id_salt(("compare", group)).auto_shrink([false, false]).show(ui, |ui| {
        Frame::new().inner_margin(Margin { left: 24, right: 24, top: 8, bottom: 40 }).show(ui, |ui| {
            let avail = ui.available_width().min(1200.0);
            ui.set_max_width(avail);
            ui.spacing_mut().item_spacing.y = 16.0;
            w::card().inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                w::text_medium(ui, format!("Prompt · sent to {} agents, each in its own worktree from {base_name} at {base}", runs.len()), 12.0, pal.muted);
                ui.add_space(4.0);
                ui.add(egui::Label::new(super::markdown::inline_job(&prompt, fs(14.0), pal.fg)).wrap());
            });
            // Run cards
            let cols = ((avail + 12.0) / 312.0).floor().clamp(1.0, runs.len() as f32) as usize;
            let cw = (avail - 12.0 * (cols as f32 - 1.0)) / cols as f32;
            let best = recommendation(app, &runs);
            for chunk in runs.chunks(cols) {
                w::row_top(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    for r in chunk {
                        run_card(app, ui, r, cw, best.as_ref().map(|b| b.0.as_str()) == Some(r.id.as_str()));
                    }
                });
            }
            // Kiln's read
            Frame::new().fill(theme::tint(pal.info, 0.06)).stroke(Stroke::new(1.0, theme::tint(pal.info, 0.25))).corner_radius(radius(theme::R_XL)).inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                w::row_top(ui, |ui| {
                    w::icon(ui, ic::SPARKLES, 16.0, pal.info);
                    ui.vertical(|ui| {
                        let text = match &best {
                            Some((_, why)) => why.clone(),
                            None => explain_no_pick(&runs),
                        };
                        let mut job = egui::text::LayoutJob::default();
                        job.append("Kiln's read: ", 0.0, egui::TextFormat::simple(theme::ui_semibold(fs(13.0)), pal.fg));
                        job.append(&text, 0.0, egui::TextFormat::simple(theme::ui(fs(13.0)), pal.fg));
                        ui.add(egui::Label::new(job).wrap());
                        ui.add_space(4.0);
                        w::text(ui, "Criteria: runs that passed their local tests first, then the smallest change. Kiln doesn't judge code quality.", 12.0, pal.muted);
                        let can_test = runs.iter().any(|r| !r.state.is_active() && r.cwd.is_some());
                        if can_test && Btn::new("Run tests in every worktree").icon(ic::TEST_TUBE).show(ui).clicked() {
                            for r in &runs {
                                if !r.state.is_active() {
                                    app.run_tests(&r.id);
                                }
                            }
                        }
                    });
                });
            });
            matrix(app, ui, &runs);
        });
    });
}

fn run_label(app: &App, r: &Thread) -> String {
    let m = app.model_label(r.provider, &r.model);
    if m.starts_with(r.provider.label()) || r.provider == ProviderKind::Claude { m } else { format!("{} · {m}", r.provider.label()) }
}

fn run_card(app: &mut App, ui: &mut Ui, r: &Thread, width: f32, best: bool) {
    let pal = p();
    let (n, a, d) = app.git.get(&r.id).map(|g| g.totals()).unwrap_or((r.files_changed, r.additions, r.deletions));
    let (st_label, glyph, color, spin) = match r.state {
        RunState::Working => ("Working", ic::CIRCLE_DASHED, pal.info, true),
        RunState::Queued => ("Queued", ic::CIRCLE_DASHED, pal.muted, false),
        RunState::NeedsApproval => ("Approval", ic::SHIELD_QUESTION, pal.warning, false),
        RunState::Failed => ("Failed", ic::CIRCLE_X, pal.danger, false),
        RunState::Cancelled => ("Stopped", ic::CIRCLE_X, pal.muted, false),
        _ => ("Done", ic::CIRCLE_CHECK, pal.success, false),
    };
    let summary = app
        .items
        .get(&r.id)
        .cloned()
        .unwrap_or_else(|| app.store.load_items(&r.id))
        .iter()
        .rev()
        .find_map(|i| if let ItemKind::Assistant { text } = &i.kind { (!text.trim().is_empty()).then(|| text.clone()) } else { None });
    Frame::new()
        .fill(pal.card)
        .stroke(Stroke::new(
            1.0,
            if best {
                theme::tint(pal.success, 0.3)
            } else if r.kept {
                theme::tint(pal.accent, 0.4)
            } else {
                pal.border_soft
            },
        ))
        .corner_radius(radius(theme::R_XL))
        .show(ui, |ui| {
            ui.set_width(width);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                Frame::new().inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
                    ui.set_width(width - 28.0);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal(|ui| {
                        w::badge(ui, r.run_label.as_deref().unwrap_or("?"), pal.fg, pal.hover);
                        w::text_medium(ui, run_label(app, r), 14.0, pal.fg);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            w::pill(ui, Some(glyph), st_label, color, 20.0, spin);
                        });
                    });
                    w::mono_text(ui, r.branch.clone().unwrap_or_else(|| "creating worktree…".into()), 12.0, pal.muted);
                    if best {
                        ui.horizontal(|ui| {
                            w::badge(ui, "Best result", pal.success, theme::tint(pal.success, 0.14));
                        });
                    }
                    if r.kept {
                        w::badge(ui, "Kept", pal.accent, theme::tint(pal.accent, 0.16));
                    }
                });
                w::hairline(ui, pal.border_faint);
                Frame::new().inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
                    egui::Grid::new(("metrics", &r.id)).num_columns(2).spacing(vec2(12.0, 10.0)).min_col_width((width - 40.0) / 2.0).show(
                        ui,
                        |ui| {
                            let tests = r.test_summary.clone().unwrap_or_else(|| "Not run".into());
                            let tc = match r.test_ok {
                                Some(true) => pal.success,
                                Some(false) => pal.danger,
                                None => pal.muted,
                            };
                            metric(ui, "Tests (local)", &tests, tc);
                            metric(ui, "Changes", &format!("+{a} −{d} · {n} file{}", if n == 1 { "" } else { "s" }), pal.fg);
                            ui.end_row();
                            let secs = r.worked_secs + r.turn_started_at.map(|s| now() - s).unwrap_or(0);
                            metric(ui, "Time", &w::duration(secs), pal.fg);
                            let usage = match (r.usage.est_cost_usd, r.usage.total_output) {
                                (Some(c), _) => format!("${c:.2} est."),
                                (None, out) if out > 0 => format!("{} tokens", fmt_tokens(r.usage.total_input + out)),
                                _ => "Not reported".into(),
                            };
                            metric(ui, "Usage", &usage, pal.fg);
                            ui.end_row();
                        },
                    );
                    if let Some(s) = &summary {
                        ui.add_space(8.0);
                        let first = s.split("\n\n").next().unwrap_or(s);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(crate::provider::short(first, 260)).font(theme::ui(fs(13.0))).color(pal.fg_soft),
                            )
                            .wrap(),
                        );
                    }
                    if let Some(e) = &r.last_error {
                        ui.add_space(6.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(crate::provider::short(e, 200)).font(theme::ui(fs(12.0))).color(pal.danger),
                            )
                            .wrap(),
                        );
                    }
                });
                w::hairline(ui, pal.border_faint);
                Frame::new().inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if !r.state.is_active() && !r.kept && Btn::primary("Keep this run").show(ui).clicked() {
                            app.keep_run(&r.id);
                        }
                        if r.state.is_active() && Btn::new("Stop").show(ui).clicked() {
                            app.stop_run(&r.id);
                        }
                        if Btn::ghost("Open run").show(ui).clicked() {
                            app.open_thread(&r.id);
                        }
                        if let Some(c) = &r.cwd
                            && Btn::ghost("Open worktree").show(ui).clicked()
                        {
                            let _ = open::that(c);
                        }
                        if !r.state.is_active()
                            && r.cwd.is_some()
                            && Btn::ghost(if app.ui.busy.contains(&format!("tests:{}", r.id)) { "Testing…" } else { "Run tests" })
                                .show(ui)
                                .clicked()
                        {
                            app.run_tests(&r.id);
                        }
                    });
                });
            });
        });
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn metric(ui: &mut Ui, label: &str, value: &str, color: egui::Color32) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        w::text(ui, label, 12.0, p().muted);
        w::text_medium(ui, value, 13.0, color);
    });
}

/// Picks a run only when there is evidence: finished runs whose local tests passed, then the
/// smallest change. Returns (thread id, explanation).
pub fn recommendation(app: &App, runs: &[Thread]) -> Option<(String, String)> {
    if runs.iter().any(|r| r.state.is_active()) {
        return None;
    }
    let passing: Vec<&Thread> = runs.iter().filter(|r| r.test_ok == Some(true)).collect();
    let best = passing.iter().min_by_key(|r| {
        let (_, a, d) = app.git.get(&r.id).map(|g| g.totals()).unwrap_or((r.files_changed, r.additions, r.deletions));
        a + d
    })?;
    let (n, a, d) = app.git.get(&best.id).map(|g| g.totals()).unwrap_or((best.files_changed, best.additions, best.deletions));
    let label = best.run_label.clone().unwrap_or_default();
    let others: Vec<String> = runs
        .iter()
        .filter(|r| r.id != best.id)
        .map(|r| {
            let l = r.run_label.clone().unwrap_or_default();
            match r.test_ok {
                Some(true) => format!("run {l} also passed with a larger change"),
                Some(false) => format!("run {l} failed its tests ({})", r.test_summary.clone().unwrap_or_default()),
                None => format!("run {l} has no test result"),
            }
        })
        .collect();
    Some((
        best.id.clone(),
        format!(
            "run {label} passed its tests ({}) with the smallest change (+{a} −{d} across {n} file{}). {}.",
            best.test_summary.clone().unwrap_or_default(),
            if n == 1 { "" } else { "s" },
            capitalize(&others.join("; "))
        ),
    ))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn explain_no_pick(runs: &[Thread]) -> String {
    if runs.iter().any(|r| r.state.is_active()) {
        "no recommendation while runs are still working.".into()
    } else if runs.iter().all(|r| r.test_ok.is_none()) {
        "no recommendation yet. Run the tests in each worktree so the results can be compared on evidence.".into()
    } else {
        "no run passed its tests, so there's nothing to recommend.".into()
    }
}

fn matrix(app: &App, ui: &mut Ui, runs: &[Thread]) {
    let pal = p();
    let mut files: BTreeMap<String, Vec<Option<(u32, u32)>>> = BTreeMap::new();
    for (i, r) in runs.iter().enumerate() {
        for c in app.git.get(&r.id).map(|g| g.changes.clone()).unwrap_or_default() {
            files.entry(c.path.clone()).or_insert_with(|| vec![None; runs.len()])[i] = Some((c.additions, c.deletions));
        }
    }
    w::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::horizontal().id_salt("matrix").show(ui, |ui| {
            Frame::new().inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                let first_w = 260.0f32.max(ui.available_width() * 0.35);
                let col_w = ((ui.available_width() - first_w) / runs.len() as f32).max(130.0);
                egui::Grid::new("files_matrix").min_col_width(col_w).spacing(vec2(0.0, 8.0)).show(ui, |ui| {
                    ui.allocate_ui(vec2(first_w, 18.0), |ui| w::text_medium(ui, "Files changed", 12.0, pal.muted));
                    for r in runs {
                        w::text_medium(
                            ui,
                            format!("Run {} · {}", r.run_label.clone().unwrap_or_default(), run_label(app, r)),
                            12.0,
                            pal.muted,
                        );
                    }
                    ui.end_row();
                    if files.is_empty() {
                        w::text(ui, "No changes yet.", 12.0, pal.faint);
                        ui.end_row();
                    }
                    for (path, cells) in &files {
                        ui.allocate_ui(vec2(first_w, 18.0), |ui| {
                            let mut job = w::path_job(path, 12.0, &pal);
                            job.wrap.max_width = first_w - 8.0;
                            job.wrap.max_rows = 1;
                            ui.add(egui::Label::new(job));
                        });
                        for c in cells {
                            match c {
                                Some((a, d)) => {
                                    w::diff_counts(ui, *a, *d, 12.0);
                                }
                                None => {
                                    ui.label(egui::RichText::new("not changed").font(theme::mono(mono_fs(12.0))).color(pal.line_no));
                                }
                            }
                        }
                        ui.end_row();
                    }
                });
            });
        });
    });
    let _ = Sense::hover();
}
