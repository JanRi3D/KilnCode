//! Native Markdown rendering (pulldown-cmark → egui layout jobs). Paragraphs are single
//! selectable labels with clickable links; code blocks get highlighting and a copy button.

use crate::highlight;
use crate::icons as ic;
use crate::theme::{self, fs, mono_fs, p, radius};
use crate::widgets as w;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Frame, Margin, Stroke, Ui};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

enum Block {
    Text { job: LayoutJob, links: Vec<(usize, usize, String)>, indent: f32, marker: Option<String>, quote: bool, heading: bool },
    Code { lang: String, text: String },
    Rule,
    Table { rows: Vec<Vec<String>> },
}

struct Inline {
    bold: bool,
    italic: bool,
    strike: bool,
    link: Option<String>,
    link_start: usize,
    heading: Option<f32>,
}

fn fmt(st: &Inline, size: f32, color: Color32) -> TextFormat {
    let size = st.heading.unwrap_or(size);
    let font = if st.bold || st.heading.is_some() { theme::ui_semibold(size) } else { theme::ui(size) };
    let mut f = TextFormat::simple(font, if st.link.is_some() { p().info } else { color });
    f.italics = st.italic;
    if st.strike {
        f.strikethrough = Stroke::new(1.0, color);
    }
    if st.link.is_some() {
        f.underline = Stroke::new(1.0, theme::tint(p().info, 0.5));
    }
    f.line_height = Some(size * 1.625);
    f
}

fn parse(md: &str, size: f32, color: Color32) -> Vec<Block> {
    let pal = p();
    let mut out = Vec::new();
    let mut job = LayoutJob::default();
    let mut links = Vec::new();
    let mut st = Inline { bold: false, italic: false, strike: false, link: None, link_start: 0, heading: None };
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut marker: Option<String> = None;
    let mut quote = 0;
    let mut code: Option<(String, String)> = None;
    let mut table: Option<Vec<Vec<String>>> = None;
    let mut cell = String::new();
    let mut chars = 0usize;
    let flush = |job: &mut LayoutJob,
                 links: &mut Vec<(usize, usize, String)>,
                 out: &mut Vec<Block>,
                 marker: &mut Option<String>,
                 depth: usize,
                 quote: bool,
                 heading: bool,
                 chars: &mut usize| {
        if !job.text.trim().is_empty() {
            let j = std::mem::take(job);
            out.push(Block::Text {
                job: j,
                links: std::mem::take(links),
                indent: depth as f32 * 18.0,
                marker: marker.take(),
                quote,
                heading,
            });
        } else {
            *job = LayoutJob::default();
            links.clear();
        }
        *chars = 0;
    };
    let push_text = |job: &mut LayoutJob, s: &str, f: TextFormat, chars: &mut usize| {
        job.append(s, 0.0, f);
        *chars += s.chars().count();
    };
    for ev in Parser::new_ext(md, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS) {
        if let Some((_, text)) = code.as_mut() {
            match ev {
                Event::Text(t) => text.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, text) = code.take().unwrap_or_default();
                    out.push(Block::Code { lang, text: text.trim_end_matches('\n').to_string() });
                }
                _ => {}
            }
            continue;
        }
        if let Some(rows) = table.as_mut() {
            match ev {
                Event::Start(Tag::TableRow) | Event::Start(Tag::TableHead) => rows.push(Vec::new()),
                Event::End(TagEnd::TableCell) => {
                    if let Some(r) = rows.last_mut() {
                        r.push(std::mem::take(&mut cell));
                    }
                }
                Event::Text(t) | Event::Code(t) => cell.push_str(&t),
                Event::End(TagEnd::Table) => {
                    out.push(Block::Table { rows: table.take().unwrap_or_default() });
                }
                _ => {}
            }
            continue;
        }
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut job, &mut links, &mut out, &mut marker, lists.len(), quote > 0, false, &mut chars);
                st.heading = Some(match level {
                    HeadingLevel::H1 => fs(20.0),
                    HeadingLevel::H2 => fs(17.0),
                    HeadingLevel::H3 => fs(15.0),
                    _ => fs(14.0),
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut job, &mut links, &mut out, &mut marker, lists.len(), quote > 0, true, &mut chars);
                st.heading = None;
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => flush(
                &mut job,
                &mut links,
                &mut out,
                &mut marker,
                lists.len().saturating_sub(1).max(if lists.is_empty() { 0 } else { lists.len() }),
                quote > 0,
                false,
                &mut chars,
            ),
            Event::Start(Tag::BlockQuote(_)) => quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => quote -= 1,
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut job, &mut links, &mut out, &mut marker, lists.len(), quote > 0, false, &mut chars);
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut job, &mut links, &mut out, &mut marker, lists.len(), quote > 0, false, &mut chars);
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut job, &mut links, &mut out, &mut marker, lists.len(), quote > 0, false, &mut chars);
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut job, &mut links, &mut out, &mut marker, lists.len(), quote > 0, false, &mut chars);
                marker = Some(match lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}.");
                        *n += 1;
                        m
                    }
                    _ => "•".into(),
                });
            }
            Event::End(TagEnd::Item) => flush(&mut job, &mut links, &mut out, &mut marker, lists.len(), quote > 0, false, &mut chars),
            Event::TaskListMarker(done) => {
                marker = Some(if done { "☑".into() } else { "☐".into() });
            }
            Event::Start(Tag::Strong) => st.bold = true,
            Event::End(TagEnd::Strong) => st.bold = false,
            Event::Start(Tag::Emphasis) => st.italic = true,
            Event::End(TagEnd::Emphasis) => st.italic = false,
            Event::Start(Tag::Strikethrough) => st.strike = true,
            Event::End(TagEnd::Strikethrough) => st.strike = false,
            Event::Start(Tag::Link { dest_url, .. }) => {
                st.link = Some(dest_url.to_string());
                st.link_start = chars;
            }
            Event::End(TagEnd::Link) => {
                if let Some(u) = st.link.take() {
                    links.push((st.link_start, chars, u));
                }
            }
            Event::Start(Tag::Table(_)) => {
                flush(&mut job, &mut links, &mut out, &mut marker, lists.len(), quote > 0, false, &mut chars);
                table = Some(Vec::new());
            }
            Event::Text(t) => push_text(&mut job, &t, fmt(&st, size, color), &mut chars),
            Event::Code(t) => {
                let mut f = TextFormat::simple(theme::mono(mono_fs(12.5)), if st.link.is_some() { pal.info } else { pal.fg });
                f.background = pal.code_bg;
                f.line_height = Some(size * 1.625);
                push_text(&mut job, &format!("\u{2009}{t}\u{2009}"), f, &mut chars);
            }
            Event::SoftBreak => push_text(&mut job, " ", fmt(&st, size, color), &mut chars),
            Event::HardBreak => push_text(&mut job, "\n", fmt(&st, size, color), &mut chars),
            Event::Rule => {
                flush(&mut job, &mut links, &mut out, &mut marker, lists.len(), quote > 0, false, &mut chars);
                out.push(Block::Rule);
            }
            Event::Html(h) | Event::InlineHtml(h) => push_text(&mut job, &h, fmt(&st, size, pal.muted), &mut chars),
            _ => {}
        }
    }
    flush(&mut job, &mut links, &mut out, &mut marker, 0, false, false, &mut chars);
    out
}

/// Renders Markdown. `wrap_code` follows the Appearance → Word wrap setting.
pub fn show(ui: &mut Ui, md: &str, size: f32, color: Color32, wrap_code: bool) {
    let blocks = parse(md, size, color);
    ui.spacing_mut().item_spacing.y = 10.0;
    for b in blocks {
        match b {
            Block::Text { mut job, links, indent, marker, quote, heading } => {
                w::row_top(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if indent > 0.0 || marker.is_some() {
                        ui.add_space(indent.max(4.0));
                    }
                    if quote {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(3.0, size * 1.6), egui::Sense::hover());
                        ui.painter().rect_filled(r, 2.0, p().border);
                    }
                    if let Some(m) = &marker {
                        ui.add(egui::Label::new(egui::RichText::new(m).font(theme::ui(size)).color(p().muted)).selectable(false));
                    }
                    if heading {
                        ui.add_space(0.0);
                    }
                    let wrap_w = ui.available_width();
                    job.wrap.max_width = wrap_w;
                    let resp = ui.add(egui::Label::new(job.clone()).wrap());
                    if !links.is_empty() {
                        link_hits(ui, &resp, job, &links);
                    }
                });
            }
            Block::Code { lang, text } => code_block(ui, &lang, &text, wrap_code),
            Block::Rule => w::hairline(ui, p().border),
            Block::Table { rows } => {
                card_frame().show(ui, |ui| {
                    egui::Grid::new(ui.next_auto_id()).striped(false).spacing(egui::vec2(16.0, 6.0)).show(ui, |ui| {
                        for (i, r) in rows.iter().enumerate() {
                            for c in r {
                                let font = if i == 0 { theme::ui_semibold(fs(13.0)) } else { theme::ui(fs(13.0)) };
                                ui.label(egui::RichText::new(c).font(font).color(if i == 0 { p().fg } else { color }));
                            }
                            ui.end_row();
                        }
                    });
                });
            }
        }
    }
}

fn link_hits(ui: &Ui, resp: &egui::Response, job: LayoutJob, links: &[(usize, usize, String)]) {
    let Some(pos) = resp.hover_pos() else { return };
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let c = galley.cursor_from_pos(pos - resp.rect.min).index.0;
    if let Some((_, _, url)) = links.iter().find(|(a, b, _)| c >= *a && c < *b) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            let _ = open::that(url);
        }
    }
}

fn card_frame() -> Frame {
    let pal = p();
    Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border_soft)).corner_radius(radius(theme::R_LG)).inner_margin(Margin::same(10))
}

pub fn code_block(ui: &mut Ui, lang: &str, text: &str, wrap: bool) {
    let pal = p();
    let font = theme::mono(mono_fs(13.0));
    Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border_soft)).corner_radius(radius(theme::R_LG)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            w::text(ui, if lang.is_empty() { "text" } else { lang }, 11.0, pal.muted);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                if w::icon_button_sized(ui, ic::COPY, "Copy code", false, 24.0, 14.0, pal.muted).clicked() {
                    ui.ctx().copy_text(text.to_string());
                }
            });
        });
        w::hairline(ui, pal.border_faint);
        let spans = highlight::code_spans(ui.ctx(), lang, text, pal.dark);
        let mut job = LayoutJob::default();
        for (i, line) in text.split('\n').enumerate() {
            highlight::line_job(&mut job, line, spans.as_ref().and_then(|s| s.get(i)), &font, pal.ctx_fg);
            job.append("\n", 0.0, TextFormat::simple(font.clone(), pal.ctx_fg));
        }
        if job.text.ends_with('\n') {
            job.text.pop();
            if let Some(s) = job.sections.last_mut() {
                s.byte_range.end = s.byte_range.end.min(egui::epaint::text::ByteIndex(job.text.len()));
            }
        }
        Frame::new().inner_margin(Margin { left: 12, right: 12, top: 8, bottom: 10 }).show(ui, |ui| {
            // Chat code blocks always wrap: a nested horizontal scroll area inside the
            // conversation's virtualized scroll area breaks egui's layout.
            let _ = wrap;
            job.wrap.max_width = ui.available_width();
            ui.add(egui::Label::new(job).wrap());
        });
    });
}

/// Inline-code-aware plain text (user bubbles): `code` spans get the code background.
pub fn inline_job(text: &str, size: f32, color: Color32) -> LayoutJob {
    let pal = p();
    let mut job = LayoutJob::default();
    for (i, part) in text.split('`').enumerate() {
        if part.is_empty() {
            continue;
        }
        let mut f = if i % 2 == 1 {
            TextFormat::simple(theme::mono(mono_fs(12.5)), pal.fg)
        } else {
            TextFormat::simple(FontId::new(size, egui::FontFamily::Proportional), color)
        };
        if i % 2 == 1 {
            f.background = pal.code_bg;
        }
        f.line_height = Some(size * 1.625);
        job.append(part, 0.0, f);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_links_and_code() {
        let md = "# Title\n\nSome **bold** and a [link](https://example.com).\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n";
        let b = parse(md, 14.0, Color32::WHITE);
        let texts = b.iter().filter(|x| matches!(x, Block::Text { .. })).count();
        assert!(texts >= 4, "heading, paragraph, two items");
        assert!(b.iter().any(|x| matches!(x, Block::Code { lang, text } if lang == "rust" && text == "fn main() {}")));
        let link = b
            .iter()
            .find_map(|x| match x {
                Block::Text { links, .. } if !links.is_empty() => Some(links[0].clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(link.2, "https://example.com");
    }
}
