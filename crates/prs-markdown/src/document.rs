//! Owned semantic representation of a Markdown document.
//!
//! Parser-specific lifetimes and event types must not escape into this module.
//! Later parser work can therefore be replaced or tested independently of
//! layout and rendering.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    title: Option<String>,
    blocks: Vec<Block>,
    source: Option<String>,
    block_metadata: Vec<BlockMetadata>,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_title(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            blocks: Vec::new(),
            source: None,
            block_metadata: Vec::new(),
        }
    }

    pub fn from_blocks(blocks: Vec<Block>) -> Self {
        let block_metadata = block_metadata_for(&blocks);
        Self {
            title: None,
            blocks,
            source: None,
            block_metadata,
        }
    }

    pub(crate) fn from_parsed(
        source: String,
        blocks: Vec<Block>,
        block_metadata: Vec<BlockMetadata>,
    ) -> Self {
        Self {
            title: None,
            blocks,
            source: Some(source),
            block_metadata,
        }
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = Some(title.into());
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Returns the original Markdown source when this document came from a
    /// parser that retains it.
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// Returns stable semantic metadata for each top-level block.
    ///
    /// The metadata is separate from [`Block`] so the semantic block variants
    /// remain convenient to construct by hand. Parser-produced documents have
    /// source spans; documents constructed with [`Document::from_blocks`] have
    /// IDs but no source spans.
    pub fn block_metadata(&self) -> &[BlockMetadata] {
        &self.block_metadata
    }

    pub fn block_metadata_at(&self, index: usize) -> Option<&BlockMetadata> {
        self.block_metadata.get(index)
    }

    pub fn blocks_mut(&mut self) -> &mut Vec<Block> {
        &mut self.blocks
    }

    pub fn push(&mut self, block: Block) {
        self.blocks.push(block);
        self.block_metadata.push(BlockMetadata {
            id: NodeId(self.block_metadata.len() as u64),
            source_span: None,
        });
    }
}

fn block_metadata_for(blocks: &[Block]) -> Vec<BlockMetadata> {
    (0..blocks.len())
        .map(|index| BlockMetadata {
            id: NodeId(index as u64),
            source_span: None,
        })
        .collect()
}

/// A stable identity for a semantic node within one parsed document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(u64);

impl NodeId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourcePosition {
    pub line: usize,
    pub column: usize,
}

impl SourcePosition {
    pub const fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceSpan {
    pub start: SourcePosition,
    pub end: SourcePosition,
}

impl SourceSpan {
    pub const fn new(start: SourcePosition, end: SourcePosition) -> Self {
        Self { start, end }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockMetadata {
    pub id: NodeId,
    pub source_span: Option<SourceSpan>,
}

/// The five GitHub-style alert categories understood by Comrak.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    pub const fn default_title(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        content: Vec<Inline>,
        anchor: Option<String>,
    },
    Paragraph(Vec<Inline>),
    List {
        ordered: bool,
        /// The source ordinal for the first item. It is `1` for unordered
        /// lists and for ordered lists without an explicit start value.
        start: usize,
        /// Whether the source list is tight. Loose lists preserve paragraph
        /// separation between item blocks during layout.
        tight: bool,
        items: Vec<ListItem>,
    },
    Quote(Vec<Block>),
    /// A GitHub-style alert. The title is kept separately because it may be
    /// overridden in the source; layout supplies the compact alert treatment.
    Alert {
        kind: AlertKind,
        title: String,
        blocks: Vec<Block>,
    },
    /// A footnote definition retained as a normal, splittable reader block.
    /// Keeping the source name makes the fallback useful even when a parser
    /// supplies no numeric reference metadata.
    FootnoteDefinition {
        name: String,
        blocks: Vec<Block>,
    },
    Table(Table),
    CodeBlock {
        language: Option<String>,
        info: Option<String>,
        code: String,
    },
    Image {
        source: String,
        alt: String,
        title: Option<String>,
    },
    Rule,
}

impl Block {
    pub fn paragraph(text: impl Into<String>) -> Self {
        Self::Paragraph(vec![Inline::Text(text.into())])
    }

    pub fn heading(level: u8, text: impl Into<String>) -> Self {
        Self::Heading {
            level: level.clamp(1, 6),
            content: vec![Inline::Text(text.into())],
            anchor: None,
        }
    }

    pub fn plain_text(&self) -> String {
        match self {
            Self::Heading { content, .. } | Self::Paragraph(content) => {
                content.iter().map(Inline::plain_text).collect()
            }
            Self::List { items, .. } => items
                .iter()
                .map(|item| {
                    item.content
                        .iter()
                        .map(Inline::plain_text)
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Quote(blocks) => blocks
                .iter()
                .map(Self::plain_text)
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Alert { title, blocks, .. } => std::iter::once(title.clone())
                .chain(blocks.iter().map(Self::plain_text))
                .collect::<Vec<_>>()
                .join(": "),
            Self::FootnoteDefinition { name, blocks } => format!(
                "[^{}]: {}",
                name,
                blocks
                    .iter()
                    .map(Self::plain_text)
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            Self::Table(table) => table
                .headers
                .iter()
                .chain(table.rows.iter().flatten())
                .map(|cell| cell.iter().map(Inline::plain_text).collect::<String>())
                .collect::<Vec<_>>()
                .join(" | "),
            Self::CodeBlock { code, .. } => code.clone(),
            Self::Image { alt, .. } => alt.clone(),
            Self::Rule => String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    pub content: Vec<Inline>,
    pub children: Vec<Block>,
    pub task: TaskState,
}

impl ListItem {
    pub fn new(content: Vec<Inline>) -> Self {
        Self {
            content,
            children: Vec::new(),
            task: TaskState::None,
        }
    }
}

/// The semantic state of a GFM task-list item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    None,
    Unchecked,
    Checked,
}

impl TaskState {
    pub const fn is_task(self) -> bool {
        !matches!(self, Self::None)
    }

    pub const fn is_checked(self) -> bool {
        matches!(self, Self::Checked)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub headers: Vec<Vec<Inline>>,
    pub rows: Vec<Vec<Vec<Inline>>>,
    pub alignments: Vec<TableAlignment>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TableAlignment {
    #[default]
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    Code(String),
    Link {
        label: Vec<Inline>,
        destination: String,
        title: Option<String>,
    },
    /// A footnote reference. `number` is Comrak's occurrence number; zero is
    /// retained as an explicit name-based fallback for unusual parser trees.
    FootnoteReference {
        name: String,
        number: u32,
    },
    Image {
        alt: String,
        source: String,
        title: Option<String>,
    },
    SoftBreak,
    HardBreak,
}

impl Inline {
    pub fn plain_text(&self) -> String {
        match self {
            Self::Text(text) | Self::Code(text) => text.clone(),
            Self::Emphasis(children) | Self::Strong(children) | Self::Strikethrough(children) => {
                children.iter().map(Self::plain_text).collect()
            }
            Self::Link { label, .. } => label.iter().map(Self::plain_text).collect(),
            Self::FootnoteReference { name, number } => {
                if *number == 0 {
                    format!("[^{name}]")
                } else {
                    format!("[{number}]")
                }
            }
            Self::Image { alt, .. } => alt.clone(),
            Self::SoftBreak | Self::HardBreak => "\n".into(),
        }
    }
}

impl From<String> for Inline {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for Inline {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}
