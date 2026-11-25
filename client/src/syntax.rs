//! Multi-language TextMate syntax highlighting powered by `syntect`.
//!
//! Provides stateful multi-line highlighting for the integrated code editor
//! (including embedded languages like `<style>` CSS and `<script>` JS inside HTML)
//! as well as lightweight line-stream highlighting for side-by-side and unified Diff views.

use std::path::Path;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// Cap full-file syntax highlighting at 8,000 lines so huge logs/dumps stay instant.
const MAX_HIGHLIGHT_LINES: usize = 8_000;
/// Cap per-line tokenizing at 4 KiB so minified single-line bundles never stall regex engines.
const MAX_LINE_BYTES: usize = 4_096;

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

fn highlight_single_line(
    highlighter: &mut HighlightLines<'_>,
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
    let Ok(ranges) = highlighter.highlight_line(sample, syntaxes) else {
        return Vec::new();
    };
    let mut spans = Vec::with_capacity(ranges.len());
    let mut offset = 0usize;
    for (style, segment) in ranges {
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
    spans
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
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut result = Vec::new();

    if text.is_empty() {
        result.push(Vec::new());
        return result;
    }

    for (idx, line_with_ending) in LinesWithEndings::from(text).enumerate() {
        let content_len = line_with_ending.trim_end_matches(['\r', '\n']).len();
        if idx >= MAX_HIGHLIGHT_LINES {
            result.push(Vec::new());
            continue;
        }
        result.push(highlight_single_line(
            &mut highlighter,
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
    let eng = engine();
    let syntax = find_syntax(&eng.syntaxes, path);
    let theme = if light {
        &eng.light_theme
    } else {
        &eng.dark_theme
    };
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut result = Vec::with_capacity(lines.len());
    let mut buf = String::new();
    for (idx, &line) in lines.iter().enumerate() {
        if idx >= MAX_HIGHLIGHT_LINES || line.is_empty() {
            result.push(Vec::new());
            continue;
        }
        let sample = truncate_to_char_boundary(line, MAX_LINE_BYTES);
        buf.clear();
        buf.push_str(sample);
        buf.push('\n');
        result.push(highlight_single_line(
            &mut highlighter,
            &eng.syntaxes,
            &buf,
            sample.len(),
            light,
        ));
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
