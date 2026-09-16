//! Bounded, stateful syntax highlighting for fenced code blocks.
//!
//! Syntect is deliberately kept behind this module. The build script filters
//! a deliberately selected set of small grammars for the languages used most
//! often in agent output and serializes the linked set; the reader only loads
//! that generated packdump.

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, FontStyle, Style, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};

const SYNTAX_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/prs-markdown-syntaxes.packdump"));

/// The deliberately supported language names and common fenced-code aliases.
pub const SUPPORTED_LANGUAGES: &[&str] = &[
    "shell/bash",
    "Rust",
    "Python",
    "JavaScript",
    "TypeScript",
    "JSON",
    "YAML",
    "TOML",
    "C",
    "C++",
    "Go",
    "HTML",
    "CSS",
    "SQL",
    "diff/patch",
    "Markdown",
];

/// A highlighted source line. Source newlines are represented by the line
/// boundary, not included in any span's text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HighlightedLine {
    pub spans: Vec<HighlightSpan>,
}

/// A code span with E-ink-friendly paint attributes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub text: String,
    /// Grayscale ink value: 0 is black, 255 is white.
    pub ink: u8,
    pub bold: bool,
    pub italic: bool,
}

impl HighlightSpan {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ink: 0,
            bold: false,
            italic: false,
        }
    }
}

/// Complete highlighted code, retaining source-line boundaries for layout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HighlightedCode {
    pub lines: Vec<HighlightedLine>,
    pub recognized: bool,
}

/// Component boundary used by layout. A future smaller device highlighter can
/// replace Syntect without exposing Syntect types to document or pagination
/// layers.
pub trait CodeHighlighter {
    fn highlight(&self, language: Option<&str>, source: &str) -> HighlightedCode;
}

/// Syntect-backed highlighter using the generated bounded syntax set.
#[derive(Clone, Copy, Debug, Default)]
pub struct SyntectHighlighter;

impl SyntectHighlighter {
    pub const fn new() -> Self {
        Self
    }

    /// Size of the serialized runtime syntax payload in bytes.
    pub const fn bundle_size() -> usize {
        SYNTAX_BYTES.len()
    }

    /// Number of grammars in the generated runtime set.
    pub fn syntax_count() -> usize {
        syntax_set().syntaxes().len()
    }
}

impl CodeHighlighter for SyntectHighlighter {
    fn highlight(&self, language: Option<&str>, source: &str) -> HighlightedCode {
        let Some(language) = language.and_then(normalize_language) else {
            return plain_code(source);
        };
        let syntax = syntax_set().find_syntax_by_token(language);
        let Some(syntax) = syntax else {
            return plain_code(source);
        };

        highlight_with_syntax(syntax, source).unwrap_or_else(|| plain_code(source))
    }
}

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(|| {
        syntect::dumps::from_uncompressed_data(SYNTAX_BYTES)
            .expect("prs-markdown generated syntax bundle must be valid")
    })
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| Theme {
        name: Some("PRS-T1 E-ink".to_owned()),
        author: Some("prs-markdown".to_owned()),
        settings: ThemeSettings {
            foreground: Some(gray(0)),
            background: Some(gray(255)),
            ..ThemeSettings::default()
        },
        scopes: vec![
            theme_item("comment", gray(128), Some(FontStyle::ITALIC)),
            theme_item("string", gray(64), None),
            theme_item("constant", gray(96), None),
            theme_item("keyword", gray(0), Some(FontStyle::BOLD)),
            theme_item("storage", gray(0), Some(FontStyle::BOLD)),
            theme_item("entity", gray(32), Some(FontStyle::BOLD)),
            theme_item("support", gray(64), None),
            theme_item("variable", gray(32), None),
            theme_item("invalid", gray(0), Some(FontStyle::BOLD)),
        ],
    })
}

const fn gray(value: u8) -> Color {
    Color {
        r: value,
        g: value,
        b: value,
        a: 255,
    }
}

fn theme_item(scope: &str, foreground: Color, font_style: Option<FontStyle>) -> ThemeItem {
    ThemeItem {
        scope: scope.parse().expect("built-in theme scope is valid"),
        style: StyleModifier {
            foreground: Some(foreground),
            background: None,
            font_style,
        },
    }
}

fn normalize_language(language: &str) -> Option<&str> {
    let language = language.trim().trim_start_matches('.');
    let token = language.split_whitespace().next().unwrap_or_default();
    match token.to_ascii_lowercase().as_str() {
        "shell" | "bash" | "sh" | "zsh" | "shell-session" | "console" | "terminal" => Some("sh"),
        "rust" | "rs" => Some("rs"),
        "python" | "py" | "python3" => Some("py"),
        "javascript" | "js" | "jsx" => Some("js"),
        "typescript" | "ts" | "tsx" => Some("ts"),
        "json" | "jsonc" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "c" | "h" => Some("c"),
        "c++" | "cpp" | "cxx" | "cc" | "hpp" => Some("cpp"),
        "go" | "golang" => Some("go"),
        "html" | "htm" | "xhtml" => Some("html"),
        "css" => Some("css"),
        "sql" => Some("sql"),
        "diff" | "patch" => Some("diff"),
        "markdown" | "md" | "mkdown" => Some("md"),
        _ => None,
    }
}

fn highlight_with_syntax(syntax: &SyntaxReference, source: &str) -> Option<HighlightedCode> {
    let mut highlighter = HighlightLines::new(syntax, theme());
    let mut lines = Vec::new();

    for source_line in source_lines(source) {
        // The generated grammars were linked with newline-aware matching.
        // Supplying a newline to the parser while omitting it from the owned
        // result preserves multiline state without displaying a synthetic byte.
        let parser_line = format!("{source_line}\n");
        let highlighted = highlighter
            .highlight_line(&parser_line, syntax_set())
            .ok()?;
        let spans = highlighted
            .into_iter()
            .filter_map(|(style, text)| {
                let text = text.strip_suffix('\n').unwrap_or(text);
                (!text.is_empty()).then(|| highlighted_span(style, text))
            })
            .collect();
        lines.push(HighlightedLine { spans });
    }

    Some(HighlightedCode {
        lines,
        recognized: true,
    })
}

fn source_lines(source: &str) -> Vec<&str> {
    if source.is_empty() {
        return vec![""];
    }
    source
        .split_inclusive('\n')
        .map(|line| line.strip_suffix('\n').unwrap_or(line))
        .collect()
}

fn highlighted_span(style: Style, text: &str) -> HighlightSpan {
    let font_style = style.font_style.bits();
    HighlightSpan {
        text: text.to_owned(),
        ink: e_ink_gray(style.foreground),
        bold: font_style & FontStyle::BOLD.bits() != 0,
        italic: font_style & FontStyle::ITALIC.bits() != 0,
    }
}

fn e_ink_gray(color: Color) -> u8 {
    let luminance =
        (u32::from(color.r) * 2126 + u32::from(color.g) * 7152 + u32::from(color.b) * 722) / 10_000;
    match luminance {
        0..=48 => 0,
        49..=128 => 96,
        129..=200 => 160,
        _ => 208,
    }
}

fn plain_code(source: &str) -> HighlightedCode {
    HighlightedCode {
        lines: source_lines(source)
            .into_iter()
            .map(|line| HighlightedLine {
                spans: if line.is_empty() {
                    Vec::new()
                } else {
                    vec![HighlightSpan::plain(line)]
                },
            })
            .collect(),
        recognized: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_bundle_is_small_and_contains_required_grammars() {
        assert!(SyntectHighlighter::bundle_size() < 300_000);
        assert!(SyntectHighlighter::syntax_count() >= 17);
        for language in [
            "bash",
            "rust",
            "python",
            "javascript",
            "typescript",
            "json",
            "yaml",
            "toml",
            "c",
            "cpp",
            "go",
            "html",
            "css",
            "sql",
            "diff",
            "markdown",
        ] {
            assert!(normalize_language(language).is_some(), "{language}");
        }
    }

    #[test]
    fn every_required_language_uses_the_selected_runtime_set() {
        let highlighter = SyntectHighlighter::new();
        for language in [
            "bash",
            "rust",
            "python",
            "javascript",
            "typescript",
            "json",
            "yaml",
            "toml",
            "c",
            "cpp",
            "go",
            "html",
            "css",
            "sql",
            "diff",
            "markdown",
        ] {
            assert!(
                highlighter
                    .highlight(Some(language), "keyword = 1")
                    .recognized,
                "{language} should resolve to a bundled grammar"
            );
        }
    }

    #[test]
    fn unknown_language_is_lossless_plain_code() {
        let output = SyntectHighlighter.highlight(Some("not-a-real-language"), "one\ntwo");
        assert!(!output.recognized);
        assert_eq!(output.lines.len(), 2);
        assert_eq!(output.lines[0].spans[0].text, "one");
        assert_eq!(output.lines[1].spans[0].text, "two");
        assert!(output
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .all(|span| span.ink == 0));
    }

    #[test]
    fn multiline_state_highlights_a_string_on_the_following_line() {
        let output = SyntectHighlighter.highlight(Some("python"), "value = \"first\nsecond\"");
        assert!(output.recognized);
        assert_eq!(output.lines.len(), 2);
        assert!(output.lines[1].spans.iter().any(|span| span.ink != 0));
    }
}
