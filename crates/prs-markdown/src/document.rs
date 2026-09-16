//! Owned semantic representation of a Markdown document.
//!
//! Parser-specific lifetimes and event types must not escape into this module.
//! Later parser work can therefore be replaced or tested independently of
//! layout and rendering.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    title: Option<String>,
    blocks: Vec<Block>,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_title(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            blocks: Vec::new(),
        }
    }

    pub fn from_blocks(blocks: Vec<Block>) -> Self {
        Self {
            title: None,
            blocks,
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

    pub fn blocks_mut(&mut self) -> &mut Vec<Block> {
        &mut self.blocks
    }

    pub fn push(&mut self, block: Block) {
        self.blocks.push(block);
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
        items: Vec<ListItem>,
    },
    Quote(Vec<Block>),
    Table(Table),
    CodeBlock {
        language: Option<String>,
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
}

impl ListItem {
    pub fn new(content: Vec<Inline>) -> Self {
        Self {
            content,
            children: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub headers: Vec<Vec<Inline>>,
    pub rows: Vec<Vec<Vec<Inline>>>,
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
