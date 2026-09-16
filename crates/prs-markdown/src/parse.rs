//! Boundary between Comrak's parser tree and the owned document IR.
//!
//! Comrak's arena and AST references are deliberately used only in this
//! module. [`ComrakParser::parse`] copies all text, destinations, metadata,
//! and source positions into [`crate::document`] values before returning.

use crate::document::{
    AlertKind, Block, BlockMetadata, Document, Inline, ListItem, NodeId, SourcePosition,
    SourceSpan, Table, TableAlignment, TaskState,
};
use comrak::nodes::{
    AlertType as ComrakAlertType, ListType, NodeValue, TableAlignment as ComrakTableAlignment,
};
use comrak::{parse_document, Arena, Node, Options};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;

pub trait MarkdownParser {
    fn parse(&self, source: &str) -> Result<Document, ParseError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    message: String,
}

impl ParseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ParseError {}

/// A CommonMark/GFM parser whose result contains no Comrak types or lifetimes.
#[derive(Clone, Debug)]
pub struct ComrakParser {
    options: Options<'static>,
}

impl Default for ComrakParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ComrakParser {
    /// Creates a parser with the GFM extensions used by modern agent output.
    pub fn new() -> Self {
        Self {
            options: gfm_options(),
        }
    }

    /// Creates a parser with caller-supplied Comrak options.
    ///
    /// The returned document remains owned and parser-independent. Callers
    /// that replace the defaults are responsible for enabling the extensions
    /// they need.
    pub fn with_options(options: Options<'static>) -> Self {
        Self { options }
    }

    pub fn parse(&self, source: &str) -> Result<Document, ParseError> {
        let arena = Arena::new();
        let root = parse_document(&arena, source, &self.options);
        let mut converter = Converter::default();
        let children: Vec<_> = root.children().collect();
        let mut blocks = Vec::new();
        let mut metadata = Vec::new();

        for node in children {
            let id = NodeId::new(converter.next_id);
            converter.next_id += 1;
            let block = converter.convert_block(node);
            if let Some(block) = block {
                blocks.push(block);
                metadata.push(BlockMetadata {
                    id,
                    source_span: Some(source_span(node)),
                });
            }
        }

        Ok(Document::from_parsed(source.to_owned(), blocks, metadata))
    }
}

impl MarkdownParser for ComrakParser {
    fn parse(&self, source: &str) -> Result<Document, ParseError> {
        ComrakParser::parse(self, source)
    }
}

/// Parses Markdown with the crate's default CommonMark/GFM configuration.
pub fn parse(source: &str) -> Result<Document, ParseError> {
    ComrakParser::new().parse(source)
}

/// Retained as a small compatibility type for callers that used the MR-01
/// architecture placeholder. New code should use [`ComrakParser`].
#[derive(Clone, Copy, Debug, Default)]
pub struct UnsupportedParser;

impl MarkdownParser for UnsupportedParser {
    fn parse(&self, _source: &str) -> Result<Document, ParseError> {
        Err(ParseError::new(
            "Markdown parsing is not implemented by UnsupportedParser",
        ))
    }
}

fn gfm_options() -> Options<'static> {
    let mut options = Options::default();
    // These are the extensions most often emitted by GitHub-facing coding
    // agents. The owned IR keeps their content readable even when a caller
    // later chooses a different parser configuration.
    options.extension.alerts = true;
    options.extension.autolink = true;
    options.extension.footnotes = true;
    options.extension.inline_footnotes = true;
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.tasklist = true;
    options
}

#[derive(Default)]
struct Converter {
    next_id: u64,
    heading_counts: HashMap<String, usize>,
}

impl Converter {
    fn convert_block(&mut self, node: Node<'_>) -> Option<Block> {
        let value = node.data().value.clone();
        match value {
            NodeValue::Paragraph => Some(Block::Paragraph(self.convert_inlines(node))),
            NodeValue::Heading(heading) => {
                let content = self.convert_inlines(node);
                Some(Block::Heading {
                    level: heading.level.clamp(1, 6),
                    anchor: Some(self.heading_anchor(&content)),
                    content,
                })
            }
            NodeValue::List(list) => {
                let items = node
                    .children()
                    .filter_map(|item| match item.data().value.clone() {
                        NodeValue::Item(_) => Some(self.convert_list_item(item, TaskState::None)),
                        NodeValue::TaskItem(task) => Some(self.convert_list_item(
                            item,
                            if task.symbol.is_some() {
                                TaskState::Checked
                            } else {
                                TaskState::Unchecked
                            },
                        )),
                        _ => None,
                    })
                    .collect();
                Some(Block::List {
                    ordered: list.list_type == ListType::Ordered,
                    items,
                })
            }
            NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
                Some(Block::Quote(self.convert_blocks(node)))
            }
            NodeValue::CodeBlock(code) => {
                let info = non_empty(code.info.trim());
                let language = info
                    .as_deref()
                    .and_then(|value| value.split_whitespace().next())
                    .map(str::to_owned);
                Some(Block::CodeBlock {
                    language,
                    info,
                    code: code.literal,
                })
            }
            NodeValue::Table(table) => Some(self.convert_table(node, table.alignments)),
            NodeValue::ThematicBreak => Some(Block::Rule),
            NodeValue::HtmlBlock(html) => Some(Block::Paragraph(vec![Inline::Text(html.literal)])),
            NodeValue::Math(math) => {
                Some(Block::Paragraph(vec![Inline::Text(math_fallback(&math))]))
            }
            NodeValue::FootnoteDefinition(definition) => Some(Block::FootnoteDefinition {
                name: definition.name,
                blocks: self.convert_blocks(node),
            }),
            NodeValue::Alert(alert) => Some(self.convert_alert(node, *alert)),
            NodeValue::BlockDirective(directive) => {
                Some(self.directive_fallback(node, &directive.info))
            }
            NodeValue::FrontMatter(text) | NodeValue::Raw(text) => {
                Some(Block::Paragraph(vec![Inline::Text(text)]))
            }
            NodeValue::DescriptionTerm => Some(Block::Paragraph(self.convert_inlines(node))),
            NodeValue::TableCell => Some(Block::Paragraph(self.convert_inlines(node))),
            NodeValue::Text(text) => Some(Block::Paragraph(vec![Inline::Text(text.into_owned())])),
            NodeValue::Item(_) | NodeValue::TaskItem(_) => self.container_fallback(node),
            NodeValue::DescriptionList
            | NodeValue::DescriptionItem(_)
            | NodeValue::DescriptionDetails
            | NodeValue::TableRow(_)
            | NodeValue::Subtext => self.container_fallback(node),
            _ => self.container_fallback(node),
        }
    }

    fn convert_blocks(&mut self, parent: Node<'_>) -> Vec<Block> {
        let children: Vec<_> = parent.children().collect();
        children
            .into_iter()
            .filter_map(|node| self.convert_block(node))
            .collect()
    }

    fn container_fallback(&mut self, node: Node<'_>) -> Option<Block> {
        let blocks = self.convert_blocks(node);
        match blocks.len() {
            0 => None,
            1 => blocks.into_iter().next(),
            _ => Some(Block::Quote(blocks)),
        }
    }

    fn convert_list_item(&mut self, node: Node<'_>, task: TaskState) -> ListItem {
        let mut blocks = self.convert_blocks(node);
        let content = match blocks.first() {
            Some(Block::Paragraph(content)) => content.clone(),
            _ => Vec::new(),
        };
        if !content.is_empty() || matches!(blocks.first(), Some(Block::Paragraph(_))) {
            blocks.remove(0);
        }
        ListItem {
            content,
            children: blocks,
            task,
        }
    }

    fn convert_table(&mut self, node: Node<'_>, alignments: Vec<ComrakTableAlignment>) -> Block {
        let mut headers = Vec::new();
        let mut rows = Vec::new();
        for row in node.children() {
            let NodeValue::TableRow(is_header) = row.data().value.clone() else {
                continue;
            };
            let cells = row
                .children()
                .filter(|cell| matches!(cell.data().value, NodeValue::TableCell))
                .map(|cell| self.convert_inlines(cell))
                .collect::<Vec<_>>();
            if is_header {
                headers = cells;
            } else {
                rows.push(cells);
            }
        }
        Block::Table(Table {
            headers,
            rows,
            alignments: alignments.into_iter().map(convert_alignment).collect(),
        })
    }

    fn convert_alert(&mut self, node: Node<'_>, alert: comrak::nodes::NodeAlert) -> Block {
        let title = alert
            .title
            .unwrap_or_else(|| alert.alert_type.default_title().to_owned());
        Block::Alert {
            kind: convert_alert_kind(alert.alert_type),
            title,
            blocks: self.convert_blocks(node),
        }
    }

    fn directive_fallback(&mut self, node: Node<'_>, info: &str) -> Block {
        let label = if info.trim().is_empty() {
            "[unsupported block directive]".to_owned()
        } else {
            format!("[unsupported block directive: {}]", info.trim())
        };
        let mut blocks = vec![Block::Paragraph(vec![Inline::Text(label)])];
        blocks.extend(self.convert_blocks(node));
        Block::Quote(blocks)
    }

    fn convert_inlines(&mut self, parent: Node<'_>) -> Vec<Inline> {
        let children: Vec<_> = parent.children().collect();
        children
            .into_iter()
            .flat_map(|node| self.convert_inline(node))
            .collect()
    }

    fn convert_inline(&mut self, node: Node<'_>) -> Vec<Inline> {
        let value = node.data().value.clone();
        match value {
            NodeValue::Text(text) => vec![Inline::Text(text.into_owned())],
            NodeValue::Code(code) => vec![Inline::Code(code.literal)],
            NodeValue::SoftBreak => vec![Inline::SoftBreak],
            NodeValue::LineBreak => vec![Inline::HardBreak],
            NodeValue::Emph => vec![Inline::Emphasis(self.convert_inlines(node))],
            NodeValue::Strong => vec![Inline::Strong(self.convert_inlines(node))],
            NodeValue::Strikethrough => {
                vec![Inline::Strikethrough(self.convert_inlines(node))]
            }
            NodeValue::Link(link) => vec![Inline::Link {
                label: self.convert_inlines(node),
                destination: link.url,
                title: non_empty(link.title),
            }],
            NodeValue::Image(image) => {
                let alt = self
                    .convert_inlines(node)
                    .into_iter()
                    .map(|inline| inline.plain_text())
                    .collect();
                vec![Inline::Image {
                    alt,
                    source: image.url,
                    title: non_empty(image.title),
                }]
            }
            NodeValue::HtmlInline(text) | NodeValue::Raw(text) => vec![Inline::Text(text)],
            NodeValue::Math(math) => vec![Inline::Text(math_fallback(&math))],
            NodeValue::WikiLink(link) => vec![Inline::Link {
                label: self.convert_inlines(node),
                destination: link.url,
                title: None,
            }],
            NodeValue::FootnoteReference(reference) => vec![Inline::FootnoteReference {
                name: reference.name,
                number: reference.ref_num,
            }],
            NodeValue::TaskItem(task) => vec![Inline::Text(if task.symbol.is_some() {
                "[x] ".to_owned()
            } else {
                "[ ] ".to_owned()
            })],
            _ => self.convert_inlines(node),
        }
    }

    fn heading_anchor(&mut self, content: &[Inline]) -> String {
        let text = content.iter().map(Inline::plain_text).collect::<String>();
        let base = slugify(&text);
        let count = self.heading_counts.entry(base.clone()).or_default();
        let anchor = if *count == 0 {
            base
        } else {
            format!("{base}-{}", *count)
        };
        *count += 1;
        anchor
    }
}

fn convert_alignment(alignment: ComrakTableAlignment) -> TableAlignment {
    match alignment {
        ComrakTableAlignment::None => TableAlignment::None,
        ComrakTableAlignment::Left => TableAlignment::Left,
        ComrakTableAlignment::Center => TableAlignment::Center,
        ComrakTableAlignment::Right => TableAlignment::Right,
    }
}

fn convert_alert_kind(kind: ComrakAlertType) -> AlertKind {
    match kind {
        ComrakAlertType::Note => AlertKind::Note,
        ComrakAlertType::Tip => AlertKind::Tip,
        ComrakAlertType::Important => AlertKind::Important,
        ComrakAlertType::Warning => AlertKind::Warning,
        ComrakAlertType::Caution => AlertKind::Caution,
    }
}

fn math_fallback(math: &comrak::nodes::NodeMath) -> String {
    let delimiter = if math.display_math { "$$" } else { "$" };
    format!("{delimiter}{}{delimiter}", math.literal)
}

fn non_empty(value: impl Into<String>) -> Option<String> {
    let value = value.into();
    (!value.is_empty()).then_some(value)
}

fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for character in text.chars() {
        if character.is_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.extend(character.to_lowercase());
        } else if character.is_whitespace() || character == '-' {
            pending_dash = true;
        }
    }
    if slug.is_empty() {
        "section".to_owned()
    } else {
        slug
    }
}

fn source_span(node: Node<'_>) -> SourceSpan {
    let sourcepos = node.data().sourcepos;
    SourceSpan::new(
        SourcePosition::new(sourcepos.start.line, sourcepos.start.column),
        SourcePosition::new(sourcepos.end.line, sourcepos.end.column),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Block;

    const AGENT_MARKDOWN: &str = include_str!("../tests/fixtures/agent-output.md");
    const MODERN_GFM: &str = include_str!("../tests/fixtures/modern-gfm.md");

    #[test]
    fn parses_nested_agent_markdown_into_owned_semantics() {
        let document = ComrakParser::new().parse(AGENT_MARKDOWN).unwrap();

        assert_eq!(document.source(), Some(AGENT_MARKDOWN));
        assert_eq!(document.block_metadata().len(), document.blocks().len());
        assert!(document
            .block_metadata()
            .iter()
            .all(|metadata| metadata.source_span.is_some()));

        let Block::Heading {
            level,
            content,
            anchor,
        } = &document.blocks()[0]
        else {
            panic!("expected heading")
        };
        assert_eq!(*level, 1);
        assert_eq!(
            content.iter().map(Inline::plain_text).collect::<String>(),
            "Release checklist"
        );
        assert_eq!(anchor.as_deref(), Some("release-checklist"));

        let Block::Paragraph(inlines) = &document.blocks()[1] else {
            panic!("expected paragraph")
        };
        assert!(matches!(inlines[0], Inline::Text(_)));
        assert!(inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Strong(_))));
        assert!(inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Emphasis(_))));
        assert!(inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Strikethrough(_))));
        assert!(inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Code(_))));
        assert!(inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Link { .. })));
        assert!(inlines.iter().any(|inline| {
            matches!(
                inline,
                Inline::Link {
                    destination,
                    title: Some(title),
                    ..
                } if destination == "docs/design.md" && title == "Design"
            )
        }));
        assert!(inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Image { .. })));
        assert!(inlines.iter().any(|inline| {
            matches!(
                inline,
                Inline::Image {
                    alt,
                    source,
                    ..
                } if alt == "diagram" && source == "img/flow.png"
            )
        }));
        assert!(inlines
            .iter()
            .any(|inline| matches!(inline, Inline::SoftBreak)));
        assert!(inlines
            .iter()
            .any(|inline| matches!(inline, Inline::HardBreak)));

        let Block::List { items, .. } = &document.blocks()[3] else {
            panic!("expected task list")
        };
        assert_eq!(items[0].task, TaskState::Checked);
        assert_eq!(items[0].children.len(), 1);
        let Block::List { items, .. } = &items[0].children[0] else {
            panic!("expected nested task list")
        };
        assert_eq!(items[0].task, TaskState::Unchecked);

        let Block::CodeBlock {
            language,
            info,
            code,
        } = &document.blocks()[5]
        else {
            panic!("expected code block")
        };
        assert_eq!(language.as_deref(), Some("rust,ignore"));
        assert_eq!(info.as_deref(), Some("rust,ignore"));
        assert!(code.contains("println"));

        let Block::Table(table) = &document.blocks()[6] else {
            panic!("expected table")
        };
        assert_eq!(table.headers.len(), 3);
        assert_eq!(table.rows.len(), 2);
        assert_eq!(
            table.alignments,
            vec![
                TableAlignment::Left,
                TableAlignment::Center,
                TableAlignment::Right
            ]
        );

        assert!(matches!(document.blocks()[7], Block::Rule));
        assert_eq!(
            document.blocks()[4].plain_text(),
            "Ship the small reader first.\nThen measure it."
        );
    }

    #[test]
    fn duplicate_and_empty_headings_get_navigation_anchors() {
        let document = parse("# Same\n\n# Same\n\n# **!**").unwrap();
        let anchors = document
            .blocks()
            .iter()
            .filter_map(|block| match block {
                Block::Heading { anchor, .. } => anchor.as_deref(),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(anchors, ["same", "same-1", "section"]);
    }

    #[test]
    fn preserves_ordered_list_semantics() {
        let document = parse("3. First\n4. Second").unwrap();
        let Block::List { ordered, items, .. } = &document.blocks()[0] else {
            panic!("expected ordered list")
        };
        assert!(*ordered);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.task == TaskState::None));
    }

    #[test]
    fn accepted_malformed_markdown_does_not_panic_conversion() {
        let malformed = include_str!("../tests/fixtures/malformed.md");
        let document = ComrakParser::new().parse(malformed).unwrap();
        assert!(!document.blocks().is_empty());
        assert_eq!(document.source(), Some(malformed));
    }

    #[test]
    fn modern_agent_gfm_constructs_remain_owned_and_visible() {
        let document = ComrakParser::new().parse(MODERN_GFM).unwrap();

        assert!(document.blocks().iter().any(|block| matches!(
            block,
            Block::Alert {
                kind: AlertKind::Note,
                title,
                ..
            } if title == "Reader policy"
        )));
        assert!(document.blocks().iter().any(|block| matches!(
            block,
            Block::Alert {
                kind: AlertKind::Warning,
                title,
                ..
            } if title == "Warning"
        )));
        assert!(document.blocks().iter().any(|block| matches!(
            block,
            Block::FootnoteDefinition { name, .. } if name == "reader"
        )));

        let paragraph_inlines = document
            .blocks()
            .iter()
            .filter_map(|block| match block {
                Block::Paragraph(inlines) => Some(inlines),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        assert!(paragraph_inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Strikethrough(_))));
        assert!(paragraph_inlines.iter().any(|inline| matches!(
            inline,
            Inline::Link { destination, .. }
                if destination == "https://github.com/dstoc/sony-prs"
        )));
        assert!(paragraph_inlines.iter().any(|inline| matches!(
            inline,
            Inline::Link { destination, .. } if destination == "http://www.example.com"
        )));
        assert!(paragraph_inlines.iter().any(|inline| matches!(
            inline,
            Inline::Link { destination, .. } if destination == "mailto:reader@example.com"
        )));
        assert!(paragraph_inlines.iter().any(|inline| matches!(
            inline,
            Inline::FootnoteReference { name, number }
                if name == "reader" && *number == 1
        )));
    }

    #[test]
    fn enabled_math_is_preserved_as_a_readable_source_fallback() {
        let mut options = Options::default();
        options.extension.math_dollars = true;
        let document = ComrakParser::with_options(options)
            .parse("Inline $x^2$ remains visible.\n")
            .unwrap();

        assert_eq!(
            document.blocks()[0].plain_text(),
            "Inline $x^2$ remains visible."
        );
    }

    #[test]
    fn enabled_block_directives_get_a_labelled_readable_fallback() {
        let mut options = Options::default();
        options.extension.block_directive = true;
        let document = ComrakParser::with_options(options)
            .parse(":::mermaid\nflowchart TD\n    A --> B\n:::\n\nAfter\n")
            .unwrap();

        assert!(document.blocks()[0]
            .plain_text()
            .contains("unsupported block directive"));
        assert!(document.blocks()[0].plain_text().contains("flowchart TD"));
        assert_eq!(document.blocks()[1].plain_text(), "After");
    }
}
