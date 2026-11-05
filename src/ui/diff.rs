//! Native unified and split diffs. Rows are virtualized: only visible rows are laid out.

use crate::app::DiffDoc;
use crate::git::{ChangeKind, FileDiff, LineKind};
use crate::highlight;
use crate::icons as ic;
use crate::model::ReviewComment;
use crate::theme::{self, fs, mono_fs, p, radius};
use crate::widgets as w;
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::collections::{HashMap, HashSet};

pub struct Opts {
    pub split: bool,
    pub wrap: bool,
    pub only_file: Option<usize>,
    pub file_headers: bool,
    pub revert: bool,
    pub comments: bool,
    pub hide_ws: bool,
}

pub enum Action {
    ToggleCollapse(String),
    ToggleViewed(usize),
    Revert(usize, usize),
    /// (file, line, old side): deleted lines anchor to the old file's numbering.
    StartComment(usize, u32, bool),
    SubmitComment(usize, u32, bool),
    CancelComment,
    SendComment(String),
    ResolveComment(String),
}

#[derive(Clone, Copy)]
enum Row {
    File(usize),
    Binary,
    Hunk(usize, usize),
    Line(usize, usize, usize),
    Split(usize, usize, Option<usize>, Option<usize>),
    Comment(usize),
    Composer(usize, u32, bool),
    Gap,
}

const LINE_H: f32 = 20.0;

fn is_ws_only(f: &FileDiff, h: usize, l: usize) -> bool {
    let line = &f.hunks[h].lines[l];
    line.kind != LineKind::Context && line.text.trim().is_empty()
}

#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    id_salt: &str,
    doc: &DiffDoc,
    opts: &Opts,
    collapsed: &HashSet<String>,
    viewed: Option<&HashMap<String, String>>,
    comments: &[ReviewComment],
    composer: Option<(usize, u32, bool)>,
    draft: &mut String,
) -> Vec<Action> {
    let pal = p();
    let mut actions = Vec::new();
    let font = theme::mono(mono_fs(12.0));
    let char_w = w::galley_width(ui, "0000000000", &font) / 10.0;
    let gutter = if opts.split { 0.0 } else { 44.0 + 40.0 + 12.0 };
    let avail = ui.available_width();

    // Flattened line offsets per hunk for highlight lookup.
    let offsets: Vec<Vec<usize>> = doc
        .files
        .iter()
        .map(|f| {
            let mut acc = 0;
            f.hunks
                .iter()
                .map(|h| {
                    let o = acc;
                    acc += h.lines.len();
                    o
                })
                .collect()
        })
        .collect();

    // Build rows.
    let mut rows: Vec<(Row, f32)> = Vec::new();
    let mut max_chars = 0usize;
    for (fi, f) in doc.files.iter().enumerate() {
        if opts.only_file.is_some_and(|o| o != fi) {
            continue;
        }
        if opts.file_headers {
            rows.push((Row::File(fi), 36.0));
        }
        if collapsed.contains(&f.path) && opts.file_headers {
            continue;
        }
        if f.binary {
            rows.push((Row::Binary, 28.0));
            continue;
        }
        let file_comments: Vec<(usize, &ReviewComment)> =
            comments.iter().enumerate().filter(|(_, c)| c.path == f.path && !c.resolved).collect();
        for (hi, h) in f.hunks.iter().enumerate() {
            rows.push((Row::Hunk(fi, hi), 26.0));
            let push_line_extras = |rows: &mut Vec<(Row, f32)>, anchor: Option<(u32, bool)>| {
                if let Some((n, old)) = anchor {
                    for (ci, c) in &file_comments {
                        if c.line == n && c.old_side == old {
                            // Box is min(560, avail-112) wide; ~7.2px per char at 13px; header + actions ≈ 78px.
                            let box_w = ((avail - 112.0).min(560.0) - 26.0).max(120.0);
                            let lines = (c.text.chars().count() as f32 * 7.2 / box_w).ceil().max(1.0);
                            rows.push((Row::Comment(*ci), 86.0 + lines * 21.0));
                        }
                    }
                    if composer == Some((fi, n, old)) {
                        rows.push((Row::Composer(fi, n, old), 124.0));
                    }
                }
            };
            if opts.split {
                let mut i = 0;
                while i < h.lines.len() {
                    if h.lines[i].kind == LineKind::Context {
                        max_chars = max_chars.max(h.lines[i].text.chars().count());
                        rows.push((Row::Split(fi, hi, Some(i), Some(i)), LINE_H));
                        push_line_extras(&mut rows, anchor_of(&h.lines[i]));
                        i += 1;
                        continue;
                    }
                    let dels: Vec<usize> = (i..h.lines.len()).take_while(|&k| h.lines[k].kind == LineKind::Del).collect();
                    let after = i + dels.len();
                    let adds: Vec<usize> = (after..h.lines.len()).take_while(|&k| h.lines[k].kind == LineKind::Add).collect();
                    let n = dels.len().max(adds.len()).max(1);
                    for k in 0..n {
                        let (l, r) = (dels.get(k).copied(), adds.get(k).copied());
                        if opts.hide_ws && l.is_none_or(|x| is_ws_only(f, hi, x)) && r.is_none_or(|x| is_ws_only(f, hi, x)) {
                            continue;
                        }
                        for x in [l, r].into_iter().flatten() {
                            max_chars = max_chars.max(h.lines[x].text.chars().count());
                        }
                        rows.push((Row::Split(fi, hi, l, r), LINE_H));
                        push_line_extras(&mut rows, l.and_then(|x| anchor_of(&h.lines[x])));
                        if r.is_some() {
                            push_line_extras(&mut rows, r.and_then(|x| anchor_of(&h.lines[x])));
                        }
                    }
                    i = after + adds.len();
                    if dels.is_empty() && adds.is_empty() {
                        i += 1;
                    }
                }
            } else {
                for (li, l) in h.lines.iter().enumerate() {
                    if opts.hide_ws && is_ws_only(f, hi, li) {
                        continue;
                    }
                    let n = l.text.chars().count();
                    max_chars = max_chars.max(n);
                    let hgt =
                        if opts.wrap { ((n as f32 * char_w) / (avail - gutter - 24.0).max(80.0)).ceil().max(1.0) * LINE_H } else { LINE_H };
                    rows.push((Row::Line(fi, hi, li), hgt));
                    push_line_extras(&mut rows, anchor_of(l));
                }
            }
        }
        rows.push((Row::Gap, 12.0));
    }
    let content_w = if opts.wrap || opts.split { avail } else { (gutter + 16.0 + max_chars as f32 * char_w + 32.0).max(avail) };
    let mut tops = Vec::with_capacity(rows.len());
    let mut y = 0.0;
    for (_, h) in &rows {
        tops.push(y);
        y += h;
    }
    let total = y + 24.0;

    egui::ScrollArea::both().id_salt(id_salt).auto_shrink([false, false]).show_viewport(ui, |ui, vp| {
        let (full, _) = ui.allocate_exact_size(vec2(content_w, total), Sense::hover());
        let origin = full.min;
        let first = tops.partition_point(|t| *t + 60.0 < vp.min.y);
        let line_tops = &tops;
        for idx in first..rows.len() {
            let top = line_tops[idx];
            if top > vp.max.y + 40.0 {
                break;
            }
            let (row, h) = rows[idx];
            // Sticky file headers are drawn at their place; long files are navigated with the file list.
            let rect = Rect::from_min_size(pos2(origin.x, origin.y + top), vec2(content_w, h));
            let vis = Rect::from_min_size(pos2(origin.x + vp.min.x, origin.y + top), vec2(avail, h));
            match row {
                Row::File(fi) => {
                    let f = &doc.files[fi];
                    ui.painter().rect_filled(vis, 0.0, pal.diff_header);
                    ui.painter().hline(vis.x_range(), vis.top() + 0.5, Stroke::new(1.0, pal.border_faint));
                    ui.painter().hline(vis.x_range(), vis.bottom() - 0.5, Stroke::new(1.0, pal.border_faint));
                    let open = !collapsed.contains(&f.path);
                    let cb = Rect::from_min_size(pos2(vis.left() + 10.0, vis.center().y - 10.0), vec2(20.0, 20.0));
                    let r = ui.interact(cb, ui.scope_id().with(("collapse", fi)), Sense::click());
                    if r.hovered() {
                        ui.painter().rect_filled(cb, radius(theme::R_SM), pal.hover);
                    }
                    w::paint_icon(ui, cb, if open { ic::CHEVRON_DOWN } else { ic::CHEVRON_RIGHT }, 14.0, pal.muted);
                    if r.clicked() {
                        actions.push(Action::ToggleCollapse(f.path.clone()));
                    }
                    let mut job = w::path_job(&f.path, 12.0, &pal);
                    if let Some(old) = f.old_path.as_ref().filter(|_| f.kind == ChangeKind::Renamed) {
                        job.append(&format!("  ← {old}"), 0.0, egui::TextFormat::simple(theme::mono(mono_fs(11.0)), pal.muted));
                    }
                    job.wrap.max_width = vis.width() - 260.0;
                    job.wrap.max_rows = 1;
                    let g = ui.fonts_mut(|fo| fo.layout_job(job));
                    ui.painter().galley(pos2(cb.right() + 8.0, vis.center().y - g.size().y / 2.0), g, pal.fg);
                    // Viewed checkbox (right)
                    let mut x = vis.right() - 12.0;
                    if let Some(v) = viewed {
                        let is_v = v.get(&f.path) == doc.fingerprints.get(fi);
                        let label = "Viewed";
                        let lf = theme::ui(fs(12.0));
                        let lw = w::galley_width(ui, label, &lf);
                        let r = Rect::from_min_max(pos2(x - lw - 22.0, vis.top() + 6.0), pos2(x, vis.bottom() - 6.0));
                        let resp = ui.interact(r, ui.scope_id().with(("viewed", fi)), Sense::click());
                        let boxr = Rect::from_center_size(pos2(r.left() + 7.0, r.center().y), vec2(14.0, 14.0));
                        checkbox(ui, boxr, is_v);
                        ui.painter().text(pos2(r.right(), r.center().y), Align2::RIGHT_CENTER, label, lf, pal.muted);
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            actions.push(Action::ToggleViewed(fi));
                        }
                        x = r.left() - 12.0;
                    }
                    let mf = theme::mono(mono_fs(12.0));
                    let del = format!("−{}", f.deletions);
                    let dw = w::galley_width(ui, &del, &mf);
                    ui.painter().text(pos2(x, vis.center().y), Align2::RIGHT_CENTER, &del, mf.clone(), pal.del);
                    ui.painter().text(pos2(x - dw - 6.0, vis.center().y), Align2::RIGHT_CENTER, format!("+{}", f.additions), mf, pal.add);
                }
                Row::Binary => {
                    ui.painter().text(
                        pos2(vis.left() + 96.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        "Binary file changed (not shown)",
                        theme::ui(fs(12.0)),
                        pal.muted,
                    );
                }
                Row::Hunk(fi, hi) => {
                    let hk = &doc.files[fi].hunks[hi];
                    let band = Rect::from_min_max(pos2(rect.left(), rect.top() + 2.0), pos2(rect.right(), rect.bottom()));
                    ui.painter().rect_filled(band, 0.0, pal.hunk_bg);
                    ui.painter().text(
                        pos2(rect.left() + if opts.split { 12.0 } else { 92.0 }, band.center().y),
                        Align2::LEFT_CENTER,
                        &hk.header,
                        font.clone(),
                        pal.muted,
                    );
                    let f = &doc.files[fi];
                    if opts.revert && !f.binary && matches!(f.kind, ChangeKind::Modified | ChangeKind::Renamed | ChangeKind::Copied) {
                        let br = Rect::from_min_size(pos2(vis.right() - 100.0, band.top() + 1.0), vec2(90.0, 22.0));
                        let r = ui.interact(br, ui.scope_id().with(("revert", fi, hi)), Sense::click());
                        if r.hovered() {
                            ui.painter().rect_filled(br, radius(theme::R_MD), pal.hover);
                        }
                        ui.painter().text(br.center(), Align2::CENTER_CENTER, "Revert hunk", theme::ui_medium(fs(12.0)), pal.ctx_fg);
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            actions.push(Action::Revert(fi, hi));
                        }
                    }
                }
                Row::Line(fi, hi, li) => {
                    let l = &doc.files[fi].hunks[hi].lines[li];
                    let (bg, sc, fg, sign) = line_style(l.kind);
                    if bg != Color32::TRANSPARENT {
                        ui.painter().rect_filled(rect, 0.0, bg);
                    }
                    let num = |n: Option<u32>| n.map(|x| x.to_string()).unwrap_or_default();
                    let ln_area = Rect::from_min_size(rect.min, vec2(84.0, LINE_H));
                    let lr = ui.interact(ln_area, ui.scope_id().with(("ln", fi, hi, li)), Sense::click());
                    let ln_col = if lr.hovered() && opts.comments { pal.info } else { pal.line_no };
                    ui.painter().text(
                        pos2(rect.left() + 38.0, rect.top() + LINE_H / 2.0),
                        Align2::RIGHT_CENTER,
                        num(l.old_no),
                        font.clone(),
                        ln_col,
                    );
                    ui.painter().text(
                        pos2(rect.left() + 78.0, rect.top() + LINE_H / 2.0),
                        Align2::RIGHT_CENTER,
                        num(l.new_no),
                        font.clone(),
                        ln_col,
                    );
                    if opts.comments
                        && lr.on_hover_text("Comment on this line").on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                        && let Some((n, old)) = anchor_of(l)
                    {
                        actions.push(Action::StartComment(fi, n, old));
                    }
                    ui.painter().text(pos2(rect.left() + 86.0, rect.top() + LINE_H / 2.0), Align2::LEFT_CENTER, sign, font.clone(), sc);
                    let flat = offsets[fi][hi] + li;
                    paint_code(
                        ui,
                        pos2(rect.left() + gutter, rect.top()),
                        &l.text,
                        doc.spans.get(fi).and_then(|s| s.get(flat)),
                        &font,
                        fg,
                        if opts.wrap { avail - gutter - 24.0 } else { f32::INFINITY },
                    );
                }
                Row::Split(fi, hi, left, right) => {
                    let hk = &doc.files[fi].hunks[hi];
                    let half = avail / 2.0;
                    for (side, li) in [(0, left), (1, right)] {
                        let x0 = vis.left() + side as f32 * half;
                        let cell = Rect::from_min_size(pos2(x0, rect.top()), vec2(half, LINE_H));
                        let Some(li) = li else {
                            ui.painter().rect_filled(cell, 0.0, pal.ctrl_bg);
                            continue;
                        };
                        let l = &hk.lines[li];
                        let (bg, sc, fg, sign) = line_style(l.kind);
                        if bg != Color32::TRANSPARENT {
                            ui.painter().rect_filled(cell, 0.0, bg);
                        }
                        let no = if side == 0 { l.old_no } else { l.new_no };
                        ui.painter().text(
                            pos2(x0 + 38.0, cell.center().y),
                            Align2::RIGHT_CENTER,
                            no.map(|n| n.to_string()).unwrap_or_default(),
                            font.clone(),
                            pal.line_no,
                        );
                        ui.painter().text(pos2(x0 + 44.0, cell.center().y), Align2::LEFT_CENTER, sign, font.clone(), sc);
                        let flat = offsets[fi][hi] + li;
                        let clip = ui.painter().clip_rect().intersect(cell);
                        let painter_ui = ui.new_child(egui::UiBuilder::new().max_rect(cell));
                        let mut cui = painter_ui;
                        cui.set_clip_rect(clip);
                        paint_code(
                            &cui,
                            pos2(x0 + 58.0, cell.top()),
                            &l.text,
                            doc.spans.get(fi).and_then(|s| s.get(flat)),
                            &font,
                            fg,
                            f32::INFINITY,
                        );
                        if opts.comments {
                            let lr = ui.interact(
                                Rect::from_min_size(cell.min, vec2(44.0, LINE_H)),
                                ui.scope_id().with(("sln", fi, hi, li, side)),
                                Sense::click(),
                            );
                            if lr.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                                && let Some((n, old)) = anchor_of(l)
                            {
                                actions.push(Action::StartComment(fi, n, old));
                            }
                        }
                    }
                    ui.painter().vline(vis.left() + half, rect.y_range(), Stroke::new(1.0, pal.border_faint));
                }
                Row::Comment(ci) => {
                    let c = &comments[ci];
                    let box_r = Rect::from_min_size(pos2(vis.left() + 96.0, rect.top() + 6.0), vec2((avail - 112.0).min(560.0), h - 16.0));
                    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(box_r));
                    w::popover(10)
                        .fill(if pal.dark { theme::hex(0x111111) } else { pal.card })
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(&mut cui, |ui| {
                            ui.set_width(box_r.width() - 26.0);
                            ui.horizontal(|ui| {
                                w::text_semibold(ui, "You", 13.0, pal.fg);
                                w::text(ui, format!("· {}", crate::widgets::ago(c.created_at)), 13.0, pal.muted);
                                // Comments belong to the revision they were made on; flag them once the file moves on.
                                let current = doc.files.iter().position(|f| f.path == c.path).and_then(|i| doc.fingerprints.get(i));
                                if current.is_some_and(|fp| *fp != c.rev) {
                                    w::badge(ui, "Outdated", pal.warning, theme::tint(pal.warning, 0.14))
                                        .on_hover_text("The file changed after this comment; the line it refers to may have moved.");
                                }
                            });
                            ui.add(egui::Label::new(super::markdown::inline_job(&c.text, fs(13.0), pal.fg)).wrap());
                            ui.horizontal(|ui| {
                                if c.sent {
                                    w::icon(ui, ic::SEND, 12.0, pal.info);
                                    w::text(ui, "Sent to the agent", 12.0, pal.info);
                                } else if w::Btn::primary("Send to agent").icon(ic::SEND).show(ui).clicked() {
                                    actions.push(Action::SendComment(c.id.clone()));
                                }
                                if w::Btn::ghost("Resolve").show(ui).clicked() {
                                    actions.push(Action::ResolveComment(c.id.clone()));
                                }
                            });
                        });
                }
                Row::Composer(fi, line, old) => {
                    let box_r = Rect::from_min_size(pos2(vis.left() + 96.0, rect.top() + 6.0), vec2((avail - 112.0).min(560.0), h - 12.0));
                    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(box_r));
                    w::popover(10).inner_margin(10).show(&mut cui, |ui| {
                        ui.set_width(box_r.width() - 22.0);
                        let te = ui.add(
                            egui::TextEdit::multiline(draft)
                                .hint_text("Leave a comment. The agent sees it next to this line.")
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .font(theme::ui(fs(13.0))),
                        );
                        if !te.has_focus() && draft.is_empty() {
                            te.request_focus();
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if w::Btn::primary("Comment").enabled(!draft.trim().is_empty()).show(ui).clicked() {
                                actions.push(Action::SubmitComment(fi, line, old));
                            }
                            if w::Btn::ghost("Cancel").show(ui).clicked() {
                                actions.push(Action::CancelComment);
                            }
                        });
                    });
                }
                Row::Gap => {}
            }
        }
    });
    actions
}

/// Where a comment on this line is anchored: the new line number, or the old one for deletions.
pub fn anchor_of(l: &crate::git::DiffLine) -> Option<(u32, bool)> {
    match l.kind {
        LineKind::Del => l.old_no.map(|n| (n, true)),
        _ => l.new_no.map(|n| (n, false)),
    }
}

fn checkbox(ui: &Ui, r: Rect, on: bool) {
    let pal = p();
    if on {
        ui.painter().rect_filled(r, radius(3), pal.accent);
        w::paint_icon(ui, r, ic::CHECK, 11.0, Color32::WHITE);
    } else {
        ui.painter().rect_stroke(r, radius(3), Stroke::new(1.0, pal.input_border), egui::StrokeKind::Inside);
    }
}

pub fn paint_checkbox(ui: &mut Ui, on: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
    checkbox(ui, r.shrink(1.0), on);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn line_style(k: LineKind) -> (Color32, Color32, Color32, &'static str) {
    let pal = p();
    match k {
        LineKind::Add => (pal.add_bg, pal.add, pal.changed_fg, "+"),
        LineKind::Del => (pal.del_bg, pal.del, pal.changed_fg, "-"),
        LineKind::Context => (Color32::TRANSPARENT, pal.line_no, pal.ctx_fg, ""),
    }
}

fn paint_code(ui: &Ui, pos: egui::Pos2, text: &str, spans: Option<&highlight::Spans>, font: &egui::FontId, fg: Color32, wrap: f32) {
    let mut job = egui::text::LayoutJob::default();
    let text = text.replace('\t', "    ");
    highlight::line_job(&mut job, &text, spans, font, fg);
    job.wrap.max_width = wrap;
    let g = ui.fonts_mut(|f| f.layout_job(job));
    let dy = (LINE_H - g.rows.first().map(|r| r.height()).unwrap_or(LINE_H)) / 2.0;
    ui.painter().galley(pos2(pos.x, pos.y + dy.max(0.0)), g, fg);
}
