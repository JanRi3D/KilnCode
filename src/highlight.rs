//! Syntax highlighting with syntect. Work happens on background threads; the UI renders
//! plain text until spans are ready and is woken with a repaint.

use egui::Color32;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

/// (byte start, byte end, color) runs for one line.
pub type Spans = Vec<(usize, usize, Color32)>;

fn syntaxes() -> &'static SyntaxSet {
    static S: OnceLock<SyntaxSet> = OnceLock::new();
    S.get_or_init(SyntaxSet::load_defaults_newlines)
}
fn themes() -> &'static ThemeSet {
    static T: OnceLock<ThemeSet> = OnceLock::new();
    T.get_or_init(ThemeSet::load_defaults)
}

fn syntax_for(hint: &str) -> Option<&'static syntect::parsing::SyntaxReference> {
    let ss = syntaxes();
    let hint = hint.trim();
    if hint.is_empty() {
        return None;
    }
    let ext = std::path::Path::new(hint).extension().and_then(|e| e.to_str()).unwrap_or(hint);
    let alias = match ext.to_ascii_lowercase().as_str() {
        "ts" | "tsx" | "typescript" | "mts" | "cts" => "js",
        "jsx" | "mjs" | "cjs" | "javascript" => "js",
        "sh" | "bash" | "zsh" | "shell" | "console" => "sh",
        "ps1" | "powershell" | "pwsh" => "sh",
        "toml" => "yaml",
        "rs" | "rust" => "rs",
        "py" | "python" => "py",
        "yml" | "yaml" => "yaml",
        "md" | "markdown" => "md",
        "json" | "jsonc" => "json",
        "c" | "h" => "c",
        "cpp" | "cc" | "hpp" | "cxx" => "cpp",
        "cs" | "csharp" => "cs",
        "go" | "golang" => "go",
        _ => ext,
    }
    .to_string();
    ss.find_syntax_by_extension(&alias).or_else(|| ss.find_syntax_by_token(&alias)).or_else(|| ss.find_syntax_by_token(hint))
}

/// Highlights consecutive lines (state carries across lines). Blocking: call off the UI thread.
pub fn lines(hint: &str, lines: &[&str], dark: bool) -> Vec<Spans> {
    let Some(syntax) = syntax_for(hint) else { return vec![Vec::new(); lines.len()] };
    let theme = &themes().themes[if dark { "base16-ocean.dark" } else { "InspiredGitHub" }];
    let mut h = HighlightLines::new(syntax, theme);
    lines
        .iter()
        .map(|line| {
            let with_nl = format!("{line}\n");
            match h.highlight_line(&with_nl, syntaxes()) {
                Ok(regions) => {
                    let mut pos = 0;
                    let mut out = Vec::new();
                    for (style, text) in regions {
                        let end = (pos + text.len()).min(line.len());
                        if end > pos {
                            let c = style.foreground;
                            out.push((pos, end, Color32::from_rgb(c.r, c.g, c.b)));
                        }
                        pos += text.len();
                    }
                    out
                }
                Err(_) => Vec::new(),
            }
        })
        .collect()
}

type Cache = Mutex<HashMap<u64, Option<Arc<Vec<Spans>>>>>;
fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Spans for a code block, computed in the background. `None` while pending.
pub fn code_spans(ctx: &egui::Context, lang: &str, code: &str, dark: bool) -> Option<Arc<Vec<Spans>>> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (lang, code, dark).hash(&mut hasher);
    let key = hasher.finish();
    let mut c = cache().lock().ok()?;
    if let Some(v) = c.get(&key) {
        return v.clone();
    }
    if c.len() > 2000 {
        c.clear();
    }
    c.insert(key, None);
    drop(c);
    let (lang, code, ctx) = (lang.to_string(), code.to_string(), ctx.clone());
    std::thread::spawn(move || {
        let ls: Vec<&str> = code.lines().collect();
        let spans = Arc::new(lines(&lang, &ls, dark));
        if let Ok(mut c) = cache().lock() {
            c.insert(key, Some(spans));
        }
        ctx.request_repaint();
    });
    None
}

/// Builds a layout job for one line from spans (falls back to `default` color).
pub fn line_job(job: &mut egui::text::LayoutJob, text: &str, spans: Option<&Spans>, font: &egui::FontId, default: Color32) {
    let fmt = |c| egui::TextFormat::simple(font.clone(), c);
    match spans.filter(|s| !s.is_empty()) {
        Some(sp) => {
            let mut pos = 0;
            for &(a, b, c) in sp {
                if a > pos && text.is_char_boundary(a) {
                    job.append(&text[pos..a], 0.0, fmt(default));
                }
                if text.is_char_boundary(a) && text.is_char_boundary(b) && b <= text.len() && a < b {
                    job.append(&text[a..b], 0.0, fmt(c));
                    pos = b;
                }
            }
            if pos < text.len() {
                job.append(&text[pos..], 0.0, fmt(default));
            }
        }
        None => job.append(text, 0.0, fmt(default)),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rust_gets_multiple_colors() {
        let sp = super::lines("main.rs", &["fn main() { let x = 1; }"], true);
        let colors: std::collections::HashSet<_> = sp[0].iter().map(|s| s.2).collect();
        assert!(colors.len() > 1);
        assert_eq!(super::lines("unknown.zzz", &["a"], true)[0].len(), 0);
    }
}
