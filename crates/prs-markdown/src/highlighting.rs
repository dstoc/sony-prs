//! Bounded, stateful syntax highlighting for fenced code blocks.
//!
//! Syntect is deliberately kept behind this module. The reader loads Syntect's
//! bundled upstream syntax definitions and normalizes the language names used
//! in Markdown fences before selecting a syntax.

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, FontStyle, Style, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// The T1 uses grayscale updates for syntax pages. Keep every secondary role
/// on a dark, explicit level so the code surface does not turn it into a
/// near-white mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EinkTokenStyle {
    ink: u8,
    bold: bool,
    italic: bool,
}

impl EinkTokenStyle {
    fn font_style(self) -> Option<FontStyle> {
        match (self.bold, self.italic) {
            (false, false) => None,
            (true, false) => Some(FontStyle::BOLD),
            (false, true) => Some(FontStyle::ITALIC),
            (true, true) => Some(FontStyle::BOLD | FontStyle::ITALIC),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EinkPalette {
    comment: EinkTokenStyle,
    string: EinkTokenStyle,
    constant: EinkTokenStyle,
    keyword: EinkTokenStyle,
    storage: EinkTokenStyle,
    entity: EinkTokenStyle,
    support: EinkTokenStyle,
    variable: EinkTokenStyle,
    invalid: EinkTokenStyle,
}

const EINK_PALETTE: EinkPalette = EinkPalette {
    comment: EinkTokenStyle {
        ink: 96,
        bold: false,
        italic: true,
    },
    string: EinkTokenStyle {
        ink: 16,
        bold: false,
        italic: false,
    },
    constant: EinkTokenStyle {
        ink: 48,
        bold: false,
        italic: false,
    },
    keyword: EinkTokenStyle {
        ink: 0,
        bold: true,
        italic: false,
    },
    storage: EinkTokenStyle {
        ink: 0,
        bold: true,
        italic: false,
    },
    entity: EinkTokenStyle {
        ink: 32,
        bold: true,
        italic: false,
    },
    support: EinkTokenStyle {
        ink: 80,
        bold: false,
        italic: false,
    },
    variable: EinkTokenStyle {
        ink: 32,
        bold: false,
        italic: false,
    },
    invalid: EinkTokenStyle {
        ink: 0,
        bold: true,
        italic: false,
    },
};

// These are the levels used by the explicit role palette. The nearest-level
// conversion still handles any non-gray Syntect color without returning a
// washed-out value that is not part of the reader's contrast policy.
const EINK_GRAY_LEVELS: &[u8] = &[0, 16, 32, 48, 64, 80, 96, 112, 128];

/// The deliberately supported language names and common fenced-code aliases.
pub const SUPPORTED_LANGUAGES: &[&str] = &[
    "shell",
    "bash",
    "sh",
    "zsh",
    "shell-session",
    "console",
    "terminal",
    "Rust",
    "rs",
    "Python",
    "py",
    "python3",
    "JavaScript",
    "js",
    "jsx",
    "TypeScript",
    "ts",
    "tsx",
    "JSON",
    "jsonc",
    "YAML",
    "yml",
    "TOML",
    "C",
    "h",
    "C++",
    "cpp",
    "cxx",
    "cc",
    "hpp",
    "Go",
    "golang",
    "HTML",
    "htm",
    "xhtml",
    "CSS",
    "SQL",
    "diff",
    "patch",
    "Markdown",
    "md",
    "mkdown",
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

/// Component boundary used by layout. A smaller device-specific highlighter
/// can replace Syntect without exposing Syntect types to document or
/// pagination layers.
pub trait CodeHighlighter {
    fn highlight(&self, language: Option<&str>, source: &str) -> HighlightedCode;
}

/// Syntect-backed highlighter using Syntect's bundled upstream syntax set.
#[derive(Clone, Copy, Debug, Default)]
pub struct SyntectHighlighter;

impl SyntectHighlighter {
    pub const fn new() -> Self {
        Self
    }

    /// Number of grammars in Syntect's bundled upstream syntax set.
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
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
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
            theme_item("comment", EINK_PALETTE.comment),
            theme_item("string", EINK_PALETTE.string),
            theme_item("constant", EINK_PALETTE.constant),
            theme_item("keyword", EINK_PALETTE.keyword),
            theme_item("storage", EINK_PALETTE.storage),
            theme_item("entity", EINK_PALETTE.entity),
            theme_item("support", EINK_PALETTE.support),
            theme_item("variable", EINK_PALETTE.variable),
            theme_item("invalid", EINK_PALETTE.invalid),
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

fn theme_item(scope: &str, style: EinkTokenStyle) -> ThemeItem {
    ThemeItem {
        scope: scope.parse().expect("built-in theme scope is valid"),
        style: StyleModifier {
            foreground: Some(gray(style.ink)),
            background: None,
            font_style: style.font_style(),
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
        // Syntect's default upstream set has no separate TypeScript grammar.
        // JavaScript is the closest real grammar and handles shared syntax.
        "typescript" | "ts" | "tsx" => Some("js"),
        "json" | "jsonc" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        // Syntect 5.3.0's default upstream set has no TOML grammar. YAML is
        // the closest bundled data-file grammar and keeps TOML fences styled
        // with a real upstream definition.
        "toml" => Some("yaml"),
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
        // The bundled grammars use newline-aware matching. Supplying a newline
        // to the parser while omitting it from the owned result preserves
        // multiline state without displaying a synthetic byte.
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
    EINK_GRAY_LEVELS
        .iter()
        .copied()
        .min_by_key(|level| u32::from(*level).abs_diff(luminance))
        .expect("the e-ink palette must contain one grayscale level")
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
    fn bundled_upstream_set_contains_every_advertised_language() {
        assert!(SyntectHighlighter::syntax_count() > SUPPORTED_LANGUAGES.len());
        let missing: Vec<_> = SUPPORTED_LANGUAGES
            .iter()
            .filter_map(|language| {
                let token = normalize_language(language).expect("advertised language normalizes");
                syntax_set()
                    .find_syntax_by_token(token)
                    .is_none()
                    .then_some((language, token))
            })
            .collect();
        assert!(missing.is_empty(), "missing upstream grammars: {missing:?}");
    }

    #[test]
    fn every_required_language_uses_the_selected_runtime_set() {
        let highlighter = SyntectHighlighter::new();
        for language in SUPPORTED_LANGUAGES {
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

    #[test]
    fn eink_palette_separates_secondary_roles_and_stays_dark_on_code_surfaces() {
        let secondary = [
            EINK_PALETTE.comment.ink,
            EINK_PALETTE.string.ink,
            EINK_PALETTE.constant.ink,
            EINK_PALETTE.support.ink,
            EINK_PALETTE.variable.ink,
        ];
        let style = crate::style::ReaderStyle::default();
        let backgrounds = [
            style.code_background.color.red,
            style.code_continuation_background.color.red,
        ];

        for (index, ink) in secondary.iter().enumerate() {
            assert!(
                secondary[..index].iter().all(|previous| previous != ink),
                "secondary palette roles must keep distinct ink levels"
            );
            for background in backgrounds {
                assert!(
                    background.saturating_sub(*ink) >= 128,
                    "ink {ink} is too light for code background {background}"
                );
            }
        }

        for level in EINK_GRAY_LEVELS {
            assert_eq!(e_ink_gray(gray(*level)), *level);
        }
        assert_ne!(e_ink_gray(gray(64)), e_ink_gray(gray(96)));
        assert_ne!(e_ink_gray(gray(96)), e_ink_gray(gray(128)));

        // Bold remains the non-color cue for structural roles that share the
        // darkest end of the palette.
        assert_eq!(EINK_PALETTE.keyword.ink, EINK_PALETTE.invalid.ink);
        assert_eq!(
            theme_item("comment", EINK_PALETTE.comment).style.font_style,
            Some(FontStyle::ITALIC)
        );
        for style in [
            EINK_PALETTE.keyword,
            EINK_PALETTE.storage,
            EINK_PALETTE.entity,
            EINK_PALETTE.invalid,
        ] {
            assert!(style.bold);
            assert_eq!(
                theme_item("test", style).style.font_style,
                style.font_style()
            );
        }
    }

    #[test]
    fn complex_language_constructs_are_lossless() {
        let rust = assert_highlighting_is_lossless(
            "rust",
            r####"let raw = r###"contains "# and // text"###;
/* outer comment
   /* nested comment */
   still a comment */
fn main() {}"####,
        );
        assert!(contains_styled_text(&rust, "contains"));
        assert!(rust.lines[2].spans.iter().all(|span| span.italic));

        let shell = assert_highlighting_is_lossless(
            "shell",
            r#"value=$(printf '%s' "${HOME:-/tmp}")
cat <<EOF
$value
EOF"#,
        );
        assert!(contains_styled_text(&shell, "printf"));
        assert!(contains_styled_text(&shell, "EOF"));

        let python = assert_highlighting_is_lossless(
            "python",
            r#"doc = """first line
second line {value}
"""
print(doc)"#,
        );
        assert!(contains_styled_text(&python, "first line"));

        let javascript = assert_highlighting_is_lossless(
            "javascript",
            "const message = `hello ${name}`;\nconst pattern = /a[b-d]+/gi;",
        );
        assert!(contains_styled_text(&javascript, "hello"));
        assert!(contains_styled_text(&javascript, "/"));

        let html = assert_highlighting_is_lossless(
            "html",
            "<style>body { color: red; }</style>\n<script>const value = `${name}`;</script>",
        );
        assert!(contains_styled_text(&html, "color"));
        assert!(contains_styled_text(&html, "const"));

        let markdown = assert_highlighting_is_lossless(
            "markdown",
            "# Heading\n\n[link](https://example.com) and `inline code`\n\n```rust\nlet value = 1;\n```",
        );
        assert!(contains_styled_text(&markdown, "Heading"));
        assert!(contains_styled_text(&markdown, "rust"));
    }

    fn assert_highlighting_is_lossless(language: &str, source: &str) -> HighlightedCode {
        let output = SyntectHighlighter.highlight(Some(language), source);
        assert!(output.recognized, "{language} should be recognized");
        let expected_lines = source_lines(source);
        let actual_lines: Vec<String> = output
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|span| span.text.as_str()).collect())
            .collect();
        assert_eq!(
            actual_lines, expected_lines,
            "{language} changed source text"
        );
        output
    }

    fn contains_styled_text(code: &HighlightedCode, text: &str) -> bool {
        code.lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.text.contains(text) && (span.ink != 0 || span.bold || span.italic))
        })
    }
}
