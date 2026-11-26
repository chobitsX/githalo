//! Multi-language TextMate syntax highlighting powered by `syntect`.
//!
//! Provides stateful multi-line highlighting for the integrated code editor
//! (including embedded languages like `<style>` CSS and `<script>` JS inside HTML)
//! as well as lightweight line-stream highlighting for side-by-side and unified Diff views.

use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use syntect::highlighting::{
    FontStyle, HighlightIterator, HighlightState, Highlighter, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// Cap full-file syntax highlighting at 8,000 lines so huge logs/dumps stay instant.
const MAX_HIGHLIGHT_LINES: usize = 8_000;
/// Cap per-stream diff highlighting lines so massive patches stay responsive.
const MAX_DIFF_HIGHLIGHT_LINES: usize = 3_000;
/// Cap per-line tokenizing at 4 KiB so minified single-line bundles never stall regex engines.
const MAX_LINE_BYTES: usize = 4_096;
/// Reset `syntect`'s parser when scope nesting exceeds this depth; `syntect` 5.3 can enter
/// pathological backtracking / non-consuming loops near its internal stack cap (100) on
/// fragmented diff hunks or unmatched brackets.
const MAX_SCOPE_DEPTH: usize = 24;
/// If consecutive lines take longer than this to parse, the internal `with_prototype` stack is
/// bloated even without deep scopes — reset state before the next line can stall. Using a
/// two-line streak avoids false resets when `syntect` lazily compiles regexes on first entry
/// into an embedded syntax (e.g. `<style>` CSS or `<script>` JS).
const SLOW_LINE_RESET_THRESHOLD: Duration = Duration::from_millis(40);
/// Hard wall-clock budget for a background full-document highlight pass.
const DOCUMENT_HIGHLIGHT_BUDGET: Duration = Duration::from_millis(600);
/// Hard wall-clock budget for a single background diff stream highlight pass.
const DIFF_HIGHLIGHT_BUDGET: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyntaxSpan {
    pub start: usize,
    pub end: usize,
    pub color: u32,
    pub bold: bool,
    pub italic: bool,
}

struct Engine {
    syntaxes: SyntaxSet,
    dark_theme: Theme,
    light_theme: Theme,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| {
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let themes = ThemeSet::load_defaults();
        let dark_theme = themes
            .themes
            .get("base16-ocean.dark")
            .or_else(|| themes.themes.get("base16-eighties.dark"))
            .or_else(|| themes.themes.values().next())
            .cloned()
            .unwrap_or_default();
        let light_theme = themes
            .themes
            .get("InspiredGitHub")
            .or_else(|| themes.themes.get("base16-ocean.light"))
            .or_else(|| themes.themes.get("Solarized (light)"))
            .or_else(|| themes.themes.values().next())
            .cloned()
            .unwrap_or_default();
        Engine {
            syntaxes,
            dark_theme,
            light_theme,
        }
    })
}

fn find_syntax<'a>(syntaxes: &'a SyntaxSet, path: &Path) -> &'a SyntaxReference {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let lower_name = name.to_ascii_lowercase();

    match lower_name.as_str() {
        "cargo.toml" => {
            if let Some(s) = syntaxes
                .find_syntax_by_extension("toml")
                .or_else(|| syntaxes.find_syntax_by_extension("ini"))
            {
                return s;
            }
        }
        "makefile" | "gnumakefile" => {
            if let Some(s) = syntaxes.find_syntax_by_name("Makefile") {
                return s;
            }
        }
        "dockerfile" | ".gitignore" | ".gitattributes" | ".ignore" | ".dockerignore" | ".env"
        | ".zshrc" | ".bashrc" => {
            if let Some(s) = syntaxes.find_syntax_by_extension("sh") {
                return s;
            }
        }
        _ => {}
    }

    if lower_name.starts_with(".env.")
        && let Some(s) = syntaxes.find_syntax_by_extension("sh")
    {
        return s;
    }

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let lower_ext = ext.to_ascii_lowercase();
        if let Some(s) = syntaxes.find_syntax_by_extension(&lower_ext) {
            return s;
        }
        let fallback_ext = match lower_ext.as_str() {
            "ts" | "tsx" | "mts" | "cts" | "jsx" | "mjs" | "cjs" => Some("js"),
            "toml" | "conf" | "cfg" | "properties" => Some("ini"),
            "jsonc" | "jsonl" | "geojson" => Some("json"),
            "kt" | "kts" | "gradle" => Some("java"),
            "swift" => Some("rs"),
            "vue" | "svelte" | "xhtml" => Some("html"),
            "plist" | "svg" => Some("xml"),
            "scss" | "sass" | "less" => Some("css"),
            "mdx" | "markdown" => Some("md"),
            "zsh" | "fish" | "bash" => Some("sh"),
            _ => None,
        };
        if let Some(ext) = fallback_ext
            && let Some(s) = syntaxes.find_syntax_by_extension(ext)
        {
            return s;
        }
    }

    syntaxes.find_syntax_plain_text()
}

/// Human-readable display label for the detected language of `path`.
pub fn language_label(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match name.as_str() {
        "dockerfile" => return "Dockerfile",
        "makefile" | "gnumakefile" => return "Makefile",
        ".gitignore" | ".gitattributes" | ".ignore" => return "Git Ignore",
        "cargo.toml" => return "TOML",
        "cargo.lock" => return "TOML (Lock)",
        _ => {}
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => "Rust",
        "ts" | "mts" | "cts" => "TypeScript",
        "tsx" => "TSX",
        "js" | "mjs" | "cjs" => "JavaScript",
        "jsx" => "JSX",
        "html" | "htm" => "HTML",
        "xml" | "plist" => "XML",
        "svg" => "SVG",
        "css" => "CSS",
        "scss" | "sass" => "SCSS",
        "less" => "Less",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "json" | "jsonc" | "jsonl" => "JSON",
        "yml" | "yaml" => "YAML",
        "toml" => "TOML",
        "ini" | "conf" | "cfg" | "properties" => "Config",
        "md" | "markdown" | "mdx" => "Markdown",
        "py" | "pyi" => "Python",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "swift" => "Swift",
        "c" | "h" => "C",
        "cc" | "cpp" | "cxx" | "hpp" => "C++",
        "cs" => "C#",
        "sh" | "bash" | "zsh" | "fish" => "Shell",
        "sql" => "SQL",
        _ => "Plain Text",
    }
}

/// Brighten very dark foreground colors slightly so every token stays crisp on `#191a1c`.
#[inline]
fn adjust_dark_fg(r: u8, g: u8, b: u8) -> u32 {
    let luma = (0.299 * f32::from(r)) + (0.587 * f32::from(g)) + (0.114 * f32::from(b));
    let (r, g, b) = if luma < 92.0 {
        let boost = (92.0 - luma) * 0.65;
        (
            (f32::from(r) + boost).min(255.0) as u8,
            (f32::from(g) + boost).min(255.0) as u8,
            (f32::from(b) + boost).min(255.0) as u8,
        )
    } else {
        (r, g, b)
    };
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// Deepen overly bright foreground tokens so comments and yellows remain crisp on `#ffffff`
/// as well as soft green/red diff backgrounds.
#[inline]
fn adjust_light_fg(r: u8, g: u8, b: u8) -> u32 {
    let luma = (0.299 * f32::from(r)) + (0.587 * f32::from(g)) + (0.114 * f32::from(b));
    let (r, g, b) = if luma > 152.0 {
        let scale = 148.0 / luma.max(1.0);
        (
            (f32::from(r) * scale).clamp(0.0, 255.0) as u8,
            (f32::from(g) * scale).clamp(0.0, 255.0) as u8,
            (f32::from(b) * scale).clamp(0.0, 255.0) as u8,
        )
    } else {
        (r, g, b)
    };
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

struct SafeLineHighlighter<'a> {
    highlighter: Highlighter<'a>,
    syntax: &'a SyntaxReference,
    parse_state: ParseState,
    highlight_state: HighlightState,
    slow_streak: u8,
}

impl<'a> SafeLineHighlighter<'a> {
    fn new(syntax: &'a SyntaxReference, theme: &'a Theme) -> Self {
        let highlighter = Highlighter::new(theme);
        let highlight_state = HighlightState::new(&highlighter, ScopeStack::new());
        Self {
            highlighter,
            syntax,
            parse_state: ParseState::new(syntax),
            highlight_state,
            slow_streak: 0,
        }
    }

    fn reset(&mut self) {
        self.parse_state = ParseState::new(self.syntax);
        self.highlight_state = HighlightState::new(&self.highlighter, ScopeStack::new());
        self.slow_streak = 0;
    }

    fn highlight_line(
        &mut self,
        syntaxes: &SyntaxSet,
        raw_line: &str,
        content_len: usize,
        light: bool,
    ) -> Vec<SyntaxSpan> {
        if content_len == 0 {
            return Vec::new();
        }
        let sample = if raw_line.len() > MAX_LINE_BYTES {
            truncate_to_char_boundary(raw_line, MAX_LINE_BYTES)
        } else {
            raw_line
        };
        let line_start = Instant::now();
        let Ok(ops) = self.parse_state.parse_line(sample, syntaxes) else {
            self.reset();
            return Vec::new();
        };
        let iter = HighlightIterator::new(
            &mut self.highlight_state,
            &ops[..],
            sample,
            &self.highlighter,
        );
        let mut spans = Vec::new();
        let mut offset = 0usize;
        for (style, segment) in iter {
            let seg_len = segment.len();
            let start = offset;
            let end = (offset + seg_len).min(content_len);
            offset += seg_len;
            if start >= content_len || start >= end {
                continue;
            }
            let color = if light {
                adjust_light_fg(style.foreground.r, style.foreground.g, style.foreground.b)
            } else {
                adjust_dark_fg(style.foreground.r, style.foreground.g, style.foreground.b)
            };
            let bold = style.font_style.contains(FontStyle::BOLD);
            let italic = style.font_style.contains(FontStyle::ITALIC);
            spans.push(SyntaxSpan {
                start,
                end,
                color,
                bold,
                italic,
            });
        }
        if line_start.elapsed() > SLOW_LINE_RESET_THRESHOLD {
            self.slow_streak = self.slow_streak.saturating_add(1);
        } else {
            self.slow_streak = 0;
        }
        if self.highlight_state.path.len() > MAX_SCOPE_DEPTH || self.slow_streak >= 2 {
            self.reset();
        }
        spans
    }
}

/// Highlight an entire document preserving multi-line syntax state (e.g. HTML `<style>` CSS,
/// `<script>` JS, multi-line comments, and multi-line strings).
///
/// Returns one `Vec<SyntaxSpan>` per line (indices match `text.split('\n')` with trailing `\r`
/// excluded from byte offsets).
pub fn highlight_document(path: &Path, text: &str) -> Vec<Vec<SyntaxSpan>> {
    highlight_document_for_theme(path, text, crate::theme::is_light())
}

/// Highlight an entire document with an explicit light/dark theme selection.
pub fn highlight_document_for_theme(path: &Path, text: &str, light: bool) -> Vec<Vec<SyntaxSpan>> {
    let eng = engine();
    let syntax = find_syntax(&eng.syntaxes, path);
    let theme = if light {
        &eng.light_theme
    } else {
        &eng.dark_theme
    };
    let mut highlighter = SafeLineHighlighter::new(syntax, theme);
    let mut result = Vec::new();

    if text.is_empty() {
        result.push(Vec::new());
        return result;
    }

    let started = Instant::now();
    let mut budget_exceeded = false;
    for (idx, line_with_ending) in LinesWithEndings::from(text).enumerate() {
        let content_len = line_with_ending.trim_end_matches(['\r', '\n']).len();
        if idx >= MAX_HIGHLIGHT_LINES || budget_exceeded {
            result.push(Vec::new());
            continue;
        }
        if idx > 0 && idx % 64 == 0 && started.elapsed() >= DOCUMENT_HIGHLIGHT_BUDGET {
            budget_exceeded = true;
            result.push(Vec::new());
            continue;
        }
        result.push(highlighter.highlight_line(
            &eng.syntaxes,
            line_with_ending,
            content_len,
            light,
        ));
    }
    if text.ends_with('\n') {
        result.push(Vec::new());
    }
    result
}

/// Highlight a sequence of code lines (without trailing newlines), suitable for Diff viewers.
pub fn highlight_code_lines(path: &Path, lines: &[&str]) -> Vec<Vec<SyntaxSpan>> {
    highlight_code_lines_for_theme(path, lines, crate::theme::is_light())
}

/// Highlight a sequence of code lines with an explicit light/dark theme selection.
pub fn highlight_code_lines_for_theme(
    path: &Path,
    lines: &[&str],
    light: bool,
) -> Vec<Vec<SyntaxSpan>> {
    let segments: Vec<(&str, bool)> = lines.iter().map(|&line| (line, false)).collect();
    highlight_code_segments_for_theme(path, &segments, light)
}

/// Highlight a sequence of segmented code lines (without trailing newlines) where each entry is
/// `(line, reset_state_before)`. Resetting parser state at diff hunk boundaries prevents
/// unmatched brackets from accumulating across disconnected hunks.
pub fn highlight_code_segments(path: &Path, lines: &[(&str, bool)]) -> Vec<Vec<SyntaxSpan>> {
    highlight_code_segments_for_theme(path, lines, crate::theme::is_light())
}

/// Highlight segmented code lines with an explicit light/dark theme selection.
pub fn highlight_code_segments_for_theme(
    path: &Path,
    lines: &[(&str, bool)],
    light: bool,
) -> Vec<Vec<SyntaxSpan>> {
    let eng = engine();
    let syntax = find_syntax(&eng.syntaxes, path);
    let theme = if light {
        &eng.light_theme
    } else {
        &eng.dark_theme
    };
    let mut highlighter = SafeLineHighlighter::new(syntax, theme);
    let mut result = Vec::with_capacity(lines.len());
    let mut buf = String::new();
    let started = Instant::now();
    let mut budget_exceeded = false;
    for (idx, &(line, reset_before)) in lines.iter().enumerate() {
        if idx >= MAX_DIFF_HIGHLIGHT_LINES || budget_exceeded || line.is_empty() {
            if reset_before && !budget_exceeded && idx < MAX_DIFF_HIGHLIGHT_LINES {
                highlighter.reset();
            }
            result.push(Vec::new());
            continue;
        }
        if reset_before {
            highlighter.reset();
        }
        if idx > 0 && idx % 32 == 0 && started.elapsed() >= DIFF_HIGHLIGHT_BUDGET {
            budget_exceeded = true;
            result.push(Vec::new());
            continue;
        }
        let sample = truncate_to_char_boundary(line, MAX_LINE_BYTES);
        buf.clear();
        buf.push_str(sample);
        buf.push('\n');
        result.push(highlighter.highlight_line(&eng.syntaxes, &buf, sample.len(), light));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_html_with_embedded_style_css_and_chinese() {
        let html = "<!doctype html>\n<html lang=\"zh-CN\">\n<head>\n<style>\n:root { --bg: #f4f6fa; }\nbody { margin: 0; /* 中文注释 */ }\n</style>\n</head>\n</html>";
        let lines = highlight_document(Path::new("index.html"), html);
        let raw_lines: Vec<&str> = html.split('\n').collect();
        assert_eq!(lines.len(), raw_lines.len());
        // Check every span stays strictly inside UTF-8 char boundaries
        for (line_text, spans) in raw_lines.iter().zip(&lines) {
            assert!(!spans.is_empty(), "expected spans for {line_text}");
            for span in spans {
                assert!(span.start < span.end);
                assert!(span.end <= line_text.len());
                assert!(line_text.is_char_boundary(span.start));
                assert!(line_text.is_char_boundary(span.end));
            }
        }
        // Embedded CSS line should produce multiple distinct token colors
        let css_colors: std::collections::BTreeSet<u32> =
            lines[4].iter().map(|s| s.color).collect();
        assert!(css_colors.len() >= 2);
    }

    #[test]
    fn highlights_rust_multiline_comment_and_minified_line_safely() {
        let rust =
            "/* 跨行中文注释\n   第二行仍然在注释中 */\npub fn main() { println!(\"你好 🚀\"); }";
        let lines = highlight_document(Path::new("src/main.rs"), rust);
        assert_eq!(lines.len(), 3);
        assert!(!lines[0].is_empty());
        assert!(!lines[1].is_empty());
        assert!(!lines[2].is_empty());

        // Minified 6KB line with Chinese characters must never panic
        let long_line = "const a = \"中文\"; ".repeat(400);
        let res = highlight_code_lines(Path::new("bundle.js"), &[&long_line]);
        assert_eq!(res.len(), 1);
        for span in &res[0] {
            assert!(long_line.is_char_boundary(span.start));
            assert!(long_line.is_char_boundary(span.end));
        }
    }

    #[test]
    fn deeply_unclosed_js_fragments_never_hang_syntect() {
        // Simulate dozens of disconnected hunks opening blocks/parens without closing them,
        // followed by arrow functions inside `useMemo(...)` — the exact pattern that previously
        // drove `syntect`'s parser stack to 100 and triggered an infinite loop.
        let mut lines: Vec<String> = Vec::new();
        for i in 0..80 {
            lines.push(format!("  if (cond_{i} && check({i})) {{"));
            lines.push(format!("    const fn_{i} = (arg = {{}}) => {{"));
        }
        lines.push("    const quickButtonMessage = useMemo(".to_owned());
        lines.push(
            "      () => showRecommendQuickButton && this.getLastRecommendQuickButtonSourceMessage(validMessages),"
                .to_owned(),
        );
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let started = Instant::now();
        let spans = highlight_code_lines(Path::new("pages/room/index.js"), &refs);
        assert_eq!(spans.len(), refs.len());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "highlighting unclosed JS fragments took too long: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn segmented_highlighting_resets_unclosed_multiline_comment_across_hunks() {
        let segments = [
            ("/* unclosed comment from hunk 1", true),
            ("   still comment in hunk 1", false),
            ("const value = 42;", true),
        ];
        let res = highlight_code_segments(Path::new("app.js"), &segments);
        assert_eq!(res.len(), 3);
        // After reset on hunk 2, `const value = 42;` must be tokenized as code (multiple colors),
        // not swallowed into the unclosed comment from hunk 1.
        let colors: std::collections::BTreeSet<u32> = res[2].iter().map(|s| s.color).collect();
        assert!(colors.len() >= 2);
    }

    #[test]
    fn light_theme_highlights_with_crisp_contrast_and_differs_from_dark() {
        let code = "// comment\npub fn greet(name: &str) -> String { format!(\"hi {name}\") }";
        let dark = highlight_document_for_theme(Path::new("lib.rs"), code, false);
        let light = highlight_document_for_theme(Path::new("lib.rs"), code, true);
        assert_eq!(dark.len(), 2);
        assert_eq!(light.len(), 2);
        assert!(!light[0].is_empty());
        assert!(!light[1].is_empty());

        // Light mode foreground tokens should stay dark enough to read cleanly on #ffffff
        for line in &light {
            for span in line {
                let r = ((span.color >> 16) & 0xff) as f32;
                let g = ((span.color >> 8) & 0xff) as f32;
                let b = (span.color & 0xff) as f32;
                let luma = 0.299 * r + 0.587 * g + 0.114 * b;
                assert!(
                    luma <= 155.0,
                    "expected crisp light-theme luma <= 155, got {luma} for #{:06x}",
                    span.color
                );
            }
        }
        assert_ne!(dark[1], light[1]);
    }
}
