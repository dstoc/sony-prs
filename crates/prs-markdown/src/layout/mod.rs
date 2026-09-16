//! Viewport-relative document layout.

use crate::document::{Block, Inline, ListItem, Table};
use crate::navigation::NavigationTarget;
use crate::style::{ReaderStyle, TextStyle};
use embedded_graphics::geometry::{Point, Size};
use embedded_graphics::primitives::Rectangle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

impl Viewport {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentLayout {
    pub viewport: Viewport,
    pub blocks: Vec<LayoutBlock>,
}

impl DocumentLayout {
    pub fn blocks(&self) -> &[LayoutBlock] {
        &self.blocks
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutBlockKind {
    Heading,
    Paragraph,
    List,
    Quote,
    Table,
    Code,
    Image,
    Rule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutBlock {
    pub kind: LayoutBlockKind,
    pub bounds: Rectangle,
    pub lines: Vec<LayoutLine>,
    pub anchor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutLine {
    pub bounds: Rectangle,
    pub fragments: Vec<LayoutFragment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutFragment {
    pub text: String,
    pub bounds: Rectangle,
    pub style: TextStyle,
    pub link: Option<NavigationTarget>,
}

pub trait TextMeasurer {
    fn measure(&self, text: &str, style: &TextStyle) -> u32;
}

/// A deterministic fallback metric for host tests and early integration.
/// Real font backends can implement [`TextMeasurer`] without changing the IR.
#[derive(Clone, Copy, Debug, Default)]
pub struct ApproximateTextMeasurer;

impl TextMeasurer for ApproximateTextMeasurer {
    fn measure(&self, text: &str, style: &TextStyle) -> u32 {
        text.chars()
            .map(|character| {
                let factor = if character.is_whitespace() {
                    1
                } else if "ilI.,'|!".contains(character) {
                    2
                } else if "mwMW@#%".contains(character) {
                    7
                } else {
                    5
                };
                (style.font_size.max(1) * factor / 10).max(1)
            })
            .sum()
    }
}

pub struct LayoutEngine<M = ApproximateTextMeasurer> {
    pub style: ReaderStyle,
    pub measurer: M,
}

impl Default for LayoutEngine<ApproximateTextMeasurer> {
    fn default() -> Self {
        Self::new(ReaderStyle::default())
    }
}

impl LayoutEngine<ApproximateTextMeasurer> {
    pub fn new(style: ReaderStyle) -> Self {
        Self {
            style,
            measurer: ApproximateTextMeasurer,
        }
    }
}

impl<M: TextMeasurer> LayoutEngine<M> {
    pub fn with_measurer(style: ReaderStyle, measurer: M) -> Self {
        Self { style, measurer }
    }

    pub fn layout(
        &self,
        document: &crate::document::Document,
        viewport: Viewport,
    ) -> DocumentLayout {
        let mut blocks = Vec::new();
        let mut cursor_y = self.style.page_padding.top as i32;
        let content_width = viewport.width.saturating_sub(
            self.style
                .page_padding
                .left
                .saturating_add(self.style.page_padding.right),
        );

        for block in document.blocks() {
            let (layout_block, next_y) = self.layout_block(
                block,
                cursor_y,
                self.style.page_padding.left as i32,
                content_width,
            );
            cursor_y = next_y;
            blocks.push(layout_block);
        }

        DocumentLayout { viewport, blocks }
    }

    fn layout_block(&self, block: &Block, start_y: i32, x: i32, width: u32) -> (LayoutBlock, i32) {
        let (kind, anchor, lines) = match block {
            Block::Heading {
                level,
                content,
                anchor,
            } => (
                LayoutBlockKind::Heading,
                anchor.clone(),
                self.inline_lines(content, self.heading_style(*level), start_y, x, width),
            ),
            Block::Paragraph(content) => (
                LayoutBlockKind::Paragraph,
                None,
                self.inline_lines(content, self.style.body, start_y, x, width),
            ),
            Block::List { ordered, items } => (
                LayoutBlockKind::List,
                None,
                self.list_lines(*ordered, items, start_y, x, width),
            ),
            Block::Quote(blocks) => (
                LayoutBlockKind::Quote,
                None,
                self.quote_lines(blocks, start_y, x, width),
            ),
            Block::Table(table) => (
                LayoutBlockKind::Table,
                None,
                self.table_lines(table, start_y, x, width),
            ),
            Block::CodeBlock { code, .. } => (
                LayoutBlockKind::Code,
                None,
                self.code_lines(code, start_y, x, width),
            ),
            Block::Image { alt, .. } => (
                LayoutBlockKind::Image,
                None,
                self.inline_lines(
                    &[Inline::Text(format!("[image: {alt}]"))],
                    self.style.body,
                    start_y,
                    x,
                    width,
                ),
            ),
            Block::Rule => (
                LayoutBlockKind::Rule,
                None,
                vec![LayoutLine {
                    bounds: Rectangle::new(
                        Point::new(x, start_y),
                        Size::new(width, self.style.body.line_height.max(1)),
                    ),
                    fragments: Vec::new(),
                }],
            ),
        };

        let bounds = block_bounds(&lines, x, start_y);
        let end_y = rect_bottom(&bounds) + self.style.paragraph_spacing as i32;
        (
            LayoutBlock {
                kind,
                bounds,
                lines,
                anchor,
            },
            end_y,
        )
    }

    fn heading_style(&self, level: u8) -> TextStyle {
        let reduction = level.saturating_sub(1) as u32 * 2;
        TextStyle {
            font_size: self
                .style
                .heading
                .font_size
                .saturating_sub(reduction)
                .max(1),
            line_height: self
                .style
                .heading
                .line_height
                .saturating_sub(reduction)
                .max(1),
            ..self.style.heading
        }
    }

    fn inline_lines(
        &self,
        inlines: &[Inline],
        base_style: TextStyle,
        start_y: i32,
        x: i32,
        width: u32,
    ) -> Vec<LayoutLine> {
        let mut spans = Vec::new();
        for inline in inlines {
            collect_spans(inline, base_style, None, &mut spans);
        }
        wrap_spans(&self.measurer, spans, start_y, x, width)
    }

    fn list_lines(
        &self,
        ordered: bool,
        items: &[ListItem],
        start_y: i32,
        x: i32,
        width: u32,
    ) -> Vec<LayoutLine> {
        let indent = self.style.list_indent.min(width);
        let mut result = Vec::new();
        let mut y = start_y;
        for (index, item) in items.iter().enumerate() {
            let marker = if ordered {
                format!("{}. ", index + 1)
            } else {
                "• ".into()
            };
            let mut spans = vec![Span {
                text: marker,
                style: self.style.body,
                link: None,
            }];
            for inline in &item.content {
                collect_spans(inline, self.style.body, None, &mut spans);
            }
            let lines = wrap_spans(
                &self.measurer,
                spans,
                y,
                x + indent as i32,
                width.saturating_sub(indent),
            );
            y = lines
                .last()
                .map(|line| rect_bottom(&line.bounds) + self.style.paragraph_spacing as i32)
                .unwrap_or(y);
            result.extend(lines);
        }
        result
    }

    fn quote_lines(&self, blocks: &[Block], start_y: i32, x: i32, width: u32) -> Vec<LayoutLine> {
        let indent = self.style.block_quote_indent.min(width);
        let mut result = Vec::new();
        let mut y = start_y;
        for block in blocks {
            let text = block.plain_text();
            let lines = self.inline_lines(
                &[Inline::Text(format!("│ {text}"))],
                self.style.body,
                y,
                x + indent as i32,
                width.saturating_sub(indent),
            );
            y = lines
                .last()
                .map(|line| rect_bottom(&line.bounds) + self.style.paragraph_spacing as i32)
                .unwrap_or(y);
            result.extend(lines);
        }
        result
    }

    fn table_lines(&self, table: &Table, start_y: i32, x: i32, width: u32) -> Vec<LayoutLine> {
        let mut rows = Vec::new();
        if !table.headers.is_empty() {
            rows.push(table.headers.clone());
        }
        rows.extend(table.rows.clone());

        let mut result = Vec::new();
        let mut y = start_y;
        for row in rows {
            let mut cells = Vec::new();
            for (index, cell) in row.iter().enumerate() {
                if index > 0 {
                    cells.push(Inline::Text(" | ".into()));
                }
                cells.extend(cell.clone());
            }
            let lines = self.inline_lines(&cells, self.style.body, y, x, width);
            y = lines
                .last()
                .map(|line| rect_bottom(&line.bounds))
                .unwrap_or(y);
            result.extend(lines);
        }
        result
    }

    fn code_lines(&self, code: &str, start_y: i32, x: i32, width: u32) -> Vec<LayoutLine> {
        self.inline_lines(
            &[Inline::Text(code.to_owned())],
            self.style.code,
            start_y,
            x,
            width,
        )
    }
}

#[derive(Clone, Debug)]
struct Span {
    text: String,
    style: TextStyle,
    link: Option<NavigationTarget>,
}

fn collect_spans(
    inline: &Inline,
    style: TextStyle,
    inherited_link: Option<NavigationTarget>,
    spans: &mut Vec<Span>,
) {
    match inline {
        Inline::Text(text) => spans.push(Span {
            text: text.clone(),
            style,
            link: inherited_link,
        }),
        Inline::Code(text) => spans.push(Span {
            text: text.clone(),
            style: TextStyle {
                code: true,
                ..style
            },
            link: inherited_link,
        }),
        Inline::Emphasis(children) => {
            for child in children {
                collect_spans(
                    child,
                    TextStyle {
                        italic: true,
                        ..style
                    },
                    inherited_link.clone(),
                    spans,
                );
            }
        }
        Inline::Strong(children) => {
            for child in children {
                collect_spans(
                    child,
                    TextStyle {
                        bold: true,
                        ..style
                    },
                    inherited_link.clone(),
                    spans,
                );
            }
        }
        Inline::Strikethrough(children) => {
            for child in children {
                collect_spans(child, style, inherited_link.clone(), spans);
            }
        }
        Inline::Link {
            label, destination, ..
        } => {
            let target = NavigationTarget::from_destination(destination);
            for child in label {
                collect_spans(child, style, Some(target.clone()), spans);
            }
        }
        Inline::Image { alt, .. } => spans.push(Span {
            text: format!("[image: {alt}]"),
            style,
            link: inherited_link,
        }),
        Inline::SoftBreak => spans.push(Span {
            text: "\n".into(),
            style,
            link: inherited_link,
        }),
        Inline::HardBreak => spans.push(Span {
            text: "\n".into(),
            style,
            link: inherited_link,
        }),
    }
}

struct LineBuilder {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    used: u32,
    fragments: Vec<LayoutFragment>,
}

impl LineBuilder {
    fn new(x: i32, y: i32, height: u32, width: u32) -> Self {
        Self {
            x,
            y,
            width,
            height: height.max(1),
            used: 0,
            fragments: Vec::new(),
        }
    }

    fn has_content(&self) -> bool {
        !self.fragments.is_empty()
    }

    fn add(
        &mut self,
        text: String,
        measured_width: u32,
        style: TextStyle,
        link: Option<NavigationTarget>,
    ) {
        let available = self.width.saturating_sub(self.used);
        let actual_width = measured_width.min(available.max(measured_width));
        self.fragments.push(LayoutFragment {
            text,
            bounds: Rectangle::new(
                Point::new(self.x + self.used as i32, self.y),
                Size::new(actual_width, style.line_height.max(1)),
            ),
            style,
            link,
        });
        self.used = self.used.saturating_add(actual_width);
        self.height = self.height.max(style.line_height.max(1));
    }

    fn finish(self) -> LayoutLine {
        LayoutLine {
            bounds: Rectangle::new(
                Point::new(self.x, self.y),
                Size::new(self.used, self.height),
            ),
            fragments: self.fragments,
        }
    }
}

fn wrap_spans<M: TextMeasurer>(
    measurer: &M,
    spans: Vec<Span>,
    start_y: i32,
    x: i32,
    width: u32,
) -> Vec<LayoutLine> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = LineBuilder::new(x, start_y, 1, width);
    let mut pending_space: Option<(TextStyle, Option<NavigationTarget>)> = None;

    for span in spans {
        let mut word = String::new();
        for character in span.text.chars() {
            if character == '\n' {
                append_word(
                    measurer,
                    &mut line,
                    &mut lines,
                    &mut pending_space,
                    word,
                    &span,
                    width,
                );
                line = finish_line(line, &mut lines);
                word = String::new();
                pending_space = None;
            } else if character.is_whitespace() {
                append_word(
                    measurer,
                    &mut line,
                    &mut lines,
                    &mut pending_space,
                    word,
                    &span,
                    width,
                );
                word = String::new();
                if line.has_content() {
                    pending_space = Some((span.style, span.link.clone()));
                }
            } else {
                if let Some((space_style, space_link)) = pending_space.take() {
                    append_token(
                        measurer,
                        &mut line,
                        &mut lines,
                        " ",
                        space_style,
                        space_link,
                        width,
                    );
                }
                word.push(character);
            }
        }
        append_word(
            measurer,
            &mut line,
            &mut lines,
            &mut pending_space,
            word,
            &span,
            width,
        );
    }

    if line.has_content() || lines.is_empty() {
        lines.push(line.finish());
    }
    lines
}

fn append_word<M: TextMeasurer>(
    measurer: &M,
    line: &mut LineBuilder,
    lines: &mut Vec<LayoutLine>,
    pending_space: &mut Option<(TextStyle, Option<NavigationTarget>)>,
    word: String,
    span: &Span,
    width: u32,
) {
    if word.is_empty() {
        return;
    }
    if let Some((space_style, space_link)) = pending_space.take() {
        append_token(measurer, line, lines, " ", space_style, space_link, width);
    }
    append_token(
        measurer,
        line,
        lines,
        &word,
        span.style,
        span.link.clone(),
        width,
    );
}

fn append_token<M: TextMeasurer>(
    measurer: &M,
    line: &mut LineBuilder,
    lines: &mut Vec<LayoutLine>,
    text: &str,
    style: TextStyle,
    link: Option<NavigationTarget>,
    width: u32,
) {
    if text == " " && !line.has_content() {
        return;
    }
    let measured = measurer.measure(text, &style);
    if line.has_content() && line.used.saturating_add(measured) > width {
        let old = std::mem::replace(
            line,
            LineBuilder::new(line.x, line.y + line.height as i32, 1, width),
        );
        lines.push(old.finish());
    }
    if measured <= width || text.chars().count() <= 1 {
        line.add(text.to_owned(), measured.min(width), style, link);
        return;
    }

    for character in text.chars() {
        let character_text = character.to_string();
        let character_width = measurer.measure(&character_text, &style).min(width);
        if line.has_content() && line.used.saturating_add(character_width) > width {
            let old = std::mem::replace(
                line,
                LineBuilder::new(line.x, line.y + line.height as i32, 1, width),
            );
            lines.push(old.finish());
        }
        line.add(character_text, character_width, style, link.clone());
    }
}

fn finish_line(line: LineBuilder, lines: &mut Vec<LayoutLine>) -> LineBuilder {
    let next_y = line.y + line.height as i32;
    let x = line.x;
    let width = line.width;
    lines.push(line.finish());
    LineBuilder::new(x, next_y, 1, width)
}

fn block_bounds(lines: &[LayoutLine], x: i32, y: i32) -> Rectangle {
    let width = lines
        .iter()
        .map(|line| line.bounds.size.width)
        .max()
        .unwrap_or(0);
    let height = lines
        .last()
        .map(|line| rect_bottom(&line.bounds).saturating_sub(y) as u32)
        .unwrap_or(0);
    Rectangle::new(Point::new(x, y), Size::new(width, height))
}

fn rect_bottom(rectangle: &Rectangle) -> i32 {
    rectangle.top_left.y + rectangle.size.height as i32
}
