//! Width-constrained, viewport-relative document layout.
//!
//! Layout deliberately does not know about pages. A [`DocumentLayout`]
//! contains every positioned line in document coordinates; [`crate::pagination`]
//! can consequently split it at legal line boundaries later. Every advance
//! used by the wrapper comes from [`TextMeasurer`]. In production that is
//! normally [`crate::typography::FontdueTextEngine`], while the deterministic
//! approximate measurer keeps the structural tests independent of font files.

use crate::document::{image_fallback, Block, Inline, ListItem, Table, TaskState};
pub use crate::geometry::Viewport;
use crate::geometry::{TASK_CHECKBOX_GAP, TASK_CHECKBOX_SIZE};
use crate::highlighting::{CodeHighlighter, SyntectHighlighter};
use crate::image::{ImageResources, RasterImage};
use crate::navigation::NavigationTarget;
use crate::style::{ReaderStyle, TextStyle};
use embedded_graphics::geometry::{Point, Size};
use embedded_graphics::primitives::Rectangle;
use std::ops::Range;

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
    Alert,
    Footnote,
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
    /// Table-only geometry used by pagination and display-list decoration.
    /// Keeping it on the layout block means repeated table headers do not
    /// need to reparse or remeasure Markdown cells.
    pub table: Option<TableLayout>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutLine {
    pub bounds: Rectangle,
    pub fragments: Vec<LayoutFragment>,
    /// The task control displayed before this line, if this is the first line
    /// of a GFM task item.
    pub task: Option<TaskState>,
    /// The document-space x coordinate of the task control.
    pub task_checkbox_x: Option<i32>,
    /// True when this line belongs to a fenced code block, including when
    /// that block is nested inside a quote, list, alert, or footnote.
    pub code: bool,
    /// True when this displayed line is a continuation produced by wrapping
    /// one source line of a fenced code block.
    pub wrapped: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutFragment {
    /// Display text without Markdown delimiters.
    pub text: String,
    pub bounds: Rectangle,
    pub style: TextStyle,
    /// A loaded raster image occupies this fragment's bounds. `text` is
    /// empty for a loaded image; unavailable images are ordinary text
    /// fragments containing their alt-text fallback.
    pub image: Option<LayoutImage>,
    /// A linked span is repeated on every line containing its visible text.
    /// Pagination turns each fragment into a separate hit region.
    pub link: Option<NavigationTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutImage {
    pub image: RasterImage,
    pub source: String,
    pub alt: String,
}

impl LayoutImage {
    pub fn new(image: RasterImage, source: impl Into<String>, alt: impl Into<String>) -> Self {
        Self {
            image,
            source: source.into(),
            alt: alt.into(),
        }
    }
}

/// The sizing pass selected for a table. Grouped tables are continued
/// vertically with the first column repeated in each group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableLayoutMode {
    Normal,
    Compact,
    Aggressive,
    Grouped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableColumnLayout {
    /// Index in the source table, before continuation groups duplicate keys.
    pub index: usize,
    pub minimum_width: u32,
    pub preferred_width: u32,
    pub alignment: crate::document::TableAlignment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableColumnGroup {
    /// Source column indexes rendered by this vertical continuation group.
    pub columns: Vec<usize>,
    /// Allocated text widths corresponding to [`Self::columns`].
    pub widths: Vec<u32>,
    pub alignments: Vec<crate::document::TableAlignment>,
    pub x: i32,
    pub width: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRowLayout {
    /// The half-open range of display lines making up this row.
    pub line_range: Range<usize>,
    pub group: usize,
    pub header: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableLayout {
    pub mode: TableLayoutMode,
    pub columns: Vec<TableColumnLayout>,
    pub groups: Vec<TableColumnGroup>,
    /// Rows are ordered in display order. A grouped table has one header row
    /// at the start of each group; repeated page headers are synthesized by
    /// the paginator and are not included in this range metadata.
    pub rows: Vec<TableRowLayout>,
}

/// The metric boundary between document layout and a font/shaping backend.
///
/// The wrapper asks this trait for every word and character advance. This
/// keeps line breaks based on the same metrics used by the eventual renderer
/// instead of a character-count estimate.
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
    code_highlighter: SyntectHighlighter,
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
            code_highlighter: SyntectHighlighter::new(),
        }
    }
}

impl<M: TextMeasurer> LayoutEngine<M> {
    pub fn with_measurer(style: ReaderStyle, measurer: M) -> Self {
        Self {
            style,
            measurer,
            code_highlighter: SyntectHighlighter::new(),
        }
    }

    /// Lay out all top-level blocks in document coordinates.
    ///
    /// `viewport` controls the available width and initial content margins.
    /// Its height is intentionally ignored here; pagination owns vertical
    /// page breaking.
    pub fn layout(
        &self,
        document: &crate::document::Document,
        viewport: Viewport,
    ) -> DocumentLayout {
        self.layout_with_images(document, viewport, &ImageResources::default())
    }

    /// Lay out a document with display-sized images resolved by the caller's
    /// resource layer. Image resolution remains outside Markdown parsing and
    /// this engine still owns only generic page geometry.
    pub fn layout_with_images(
        &self,
        document: &crate::document::Document,
        viewport: Viewport,
        images: &ImageResources,
    ) -> DocumentLayout {
        let mut blocks = Vec::new();
        let mut cursor_y = self.style.page_padding.top as i32;
        let content_width = viewport.width.saturating_sub(
            self.style
                .page_padding
                .left
                .saturating_add(self.style.page_padding.right),
        );
        let content_x = self.style.page_padding.left as i32;
        let image_height = viewport.height.saturating_sub(
            self.style
                .page_padding
                .top
                .saturating_add(self.style.page_padding.bottom),
        );

        for block in document.blocks() {
            cursor_y = cursor_y.saturating_add(self.spacing_before(block));
            let (layout_block, block_bottom) = self.layout_block(
                block,
                cursor_y,
                content_x,
                content_width,
                image_height,
                images,
            );
            cursor_y = block_bottom.saturating_add(self.spacing_after(block));
            blocks.push(layout_block);
        }

        DocumentLayout { viewport, blocks }
    }

    fn layout_block(
        &self,
        block: &Block,
        start_y: i32,
        x: i32,
        width: u32,
        image_height: u32,
        images: &ImageResources,
    ) -> (LayoutBlock, i32) {
        let (kind, anchor, lines, table) =
            self.layout_content(block, start_y, x, width, image_height, images);
        let bounds = block_bounds(&lines, x, start_y);
        let bottom = rect_bottom(&bounds);
        (
            LayoutBlock {
                kind,
                bounds,
                lines,
                anchor,
                table,
            },
            bottom,
        )
    }

    fn layout_content(
        &self,
        block: &Block,
        start_y: i32,
        x: i32,
        width: u32,
        image_height: u32,
        images: &ImageResources,
    ) -> (
        LayoutBlockKind,
        Option<String>,
        Vec<LayoutLine>,
        Option<TableLayout>,
    ) {
        match block {
            Block::Heading {
                level,
                content,
                anchor,
            } => (
                LayoutBlockKind::Heading,
                anchor.clone(),
                self.inline_lines_with_images(
                    content,
                    self.heading_style(*level),
                    start_y,
                    x,
                    width,
                    image_height,
                    images,
                ),
                None,
            ),
            Block::Paragraph(content) => {
                let lines = self.inline_lines_with_images(
                    content,
                    self.style.body,
                    start_y,
                    x,
                    width,
                    image_height,
                    images,
                );
                let standalone_image = content.len() == 1
                    && matches!(&content[0], Inline::Image { source, .. } if images.image(source).is_some());
                (
                    if standalone_image {
                        LayoutBlockKind::Image
                    } else {
                        LayoutBlockKind::Paragraph
                    },
                    None,
                    lines,
                    None,
                )
            }
            Block::List {
                ordered,
                start,
                tight,
                items,
            } => (
                LayoutBlockKind::List,
                None,
                self.list_lines_with_images(
                    *ordered,
                    *start,
                    *tight,
                    items,
                    start_y,
                    x,
                    width,
                    image_height,
                    images,
                ),
                None,
            ),
            Block::Quote(blocks) => (
                LayoutBlockKind::Quote,
                None,
                self.quote_lines(blocks, start_y, x, width, image_height, images),
                None,
            ),
            Block::Alert { title, blocks, .. } => (
                LayoutBlockKind::Alert,
                None,
                self.alert_lines(title, blocks, start_y, x, width, image_height, images),
                None,
            ),
            Block::FootnoteDefinition { name, blocks } => (
                LayoutBlockKind::Footnote,
                None,
                self.footnote_lines(name, blocks, start_y, x, width, image_height, images),
                None,
            ),
            Block::Table(table) => {
                let (lines, table_layout) =
                    self.table_lines(table, start_y, x, width, image_height, images);
                (LayoutBlockKind::Table, None, lines, Some(table_layout))
            }
            // Syntax highlighting is a separate concern. Preserve code text,
            // line breaks, indentation, and a monospace style here.
            Block::CodeBlock { language, code, .. } => (
                LayoutBlockKind::Code,
                None,
                self.code_lines(language.as_deref(), code, start_y, x, width),
                None,
            ),
            Block::Image { source, alt, .. } => {
                let lines = if let Some(image) = images.image(source) {
                    let image = image.fitted(width, image_height);
                    let bounds = Rectangle::new(
                        Point::new(x, start_y),
                        Size::new(image.width(), image.height()),
                    );
                    vec![LayoutLine {
                        bounds,
                        fragments: vec![LayoutFragment {
                            text: String::new(),
                            bounds,
                            style: self.style.body,
                            image: Some(LayoutImage::new(image, source, alt)),
                            link: None,
                        }],
                        task: None,
                        task_checkbox_x: None,
                        code: false,
                        wrapped: false,
                    }]
                } else {
                    self.inline_lines_with_images(
                        &[Inline::Text(image_fallback(alt))],
                        self.style.body,
                        start_y,
                        x,
                        width,
                        image_height,
                        images,
                    )
                };
                (LayoutBlockKind::Image, None, lines, None)
            }
            Block::Rule => (
                LayoutBlockKind::Rule,
                None,
                vec![LayoutLine {
                    bounds: Rectangle::new(
                        Point::new(x, start_y),
                        Size::new(width, self.style.body.line_height.max(1)),
                    ),
                    fragments: Vec::new(),
                    task: None,
                    task_checkbox_x: None,
                    code: false,
                    wrapped: false,
                }],
                None,
            ),
        }
    }

    fn heading_style(&self, level: u8) -> TextStyle {
        // One configured heading style gives the reader a compact, coherent
        // hierarchy without embedding presentation in the semantic IR.
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

    fn spacing_before(&self, block: &Block) -> i32 {
        if matches!(block, Block::Heading { .. }) {
            self.style.heading_spacing_before as i32
        } else {
            0
        }
    }

    fn spacing_after(&self, block: &Block) -> i32 {
        if matches!(block, Block::Heading { .. }) {
            self.style.heading_spacing_after as i32
        } else {
            self.style.paragraph_spacing as i32
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn inline_lines_with_images(
        &self,
        inlines: &[Inline],
        base_style: TextStyle,
        start_y: i32,
        x: i32,
        width: u32,
        image_height: u32,
        images: &ImageResources,
    ) -> Vec<LayoutLine> {
        let mut spans = Vec::new();
        for inline in inlines {
            collect_spans(inline, base_style, None, image_height, images, &mut spans);
        }
        wrap_spans(
            &self.measurer,
            spans,
            start_y,
            x,
            width,
            base_style.line_height,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn list_lines_with_images(
        &self,
        ordered: bool,
        start: usize,
        tight: bool,
        items: &[ListItem],
        start_y: i32,
        x: i32,
        width: u32,
        image_height: u32,
        images: &ImageResources,
    ) -> Vec<LayoutLine> {
        let mut result = Vec::new();
        self.append_list_lines_with_images(
            ordered,
            start,
            tight,
            items,
            start_y,
            x,
            width,
            image_height,
            images,
            &mut result,
        );
        if result.is_empty() {
            result.push(empty_line(x, start_y, self.style.body.line_height));
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn append_list_lines_with_images(
        &self,
        ordered: bool,
        start: usize,
        tight: bool,
        items: &[ListItem],
        start_y: i32,
        x: i32,
        width: u32,
        image_height: u32,
        images: &ImageResources,
        output: &mut Vec<LayoutLine>,
    ) -> i32 {
        let indent = self.style.list_indent.min(width);
        let item_x = x.saturating_add(indent as i32);
        let item_width = width.saturating_sub(indent);
        let marker_width = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let marker = list_marker(ordered, start, index);
                let marker_width = self.measurer.measure(&marker, &self.style.body);
                if item.task.is_task() {
                    let checkbox_width = TASK_CHECKBOX_SIZE.saturating_add(TASK_CHECKBOX_GAP);
                    if ordered {
                        marker_width.saturating_add(checkbox_width)
                    } else {
                        marker_width.max(checkbox_width)
                    }
                } else {
                    marker_width
                }
            })
            .max()
            .unwrap_or(0)
            // Preserve the narrow-viewport progress guarantee. If the whole
            // item box is narrower than its marker, the marker is clipped by
            // the same content box as every other fragment.
            .min(item_width);
        let text_x = item_x.saturating_add(marker_width as i32);
        let text_width = item_width.saturating_sub(marker_width);
        let mut y = start_y;

        for (index, item) in items.iter().enumerate() {
            let marker = if item.task.is_task() && !ordered {
                String::new()
            } else {
                list_marker(ordered, start, index)
            };
            let task_checkbox_x = if item.task.is_task() {
                Some(
                    text_x
                        .saturating_sub(
                            TASK_CHECKBOX_SIZE.saturating_add(TASK_CHECKBOX_GAP) as i32,
                        ),
                )
            } else {
                None
            };

            let mut first_child = None;
            if item.content.is_empty() {
                // A block-only list item has no inline line to carry its
                // marker. Attach the marker to the first rendered child line
                // so quotes, headings, code, images, and nested lists keep
                // the marker beside their first visible content.
                if let Some(child) = item.children.first() {
                    y = y.saturating_add(self.spacing_before(child));
                    let (_, _, mut lines, _) =
                        self.layout_content(child, y, text_x, text_width, image_height, images);
                    prepend_list_marker(
                        &mut lines,
                        &marker,
                        item_x,
                        marker_width,
                        self.style.body,
                        self.style.body.line_height,
                    );
                    if let (Some(first), Some(checkbox_x)) = (lines.first_mut(), task_checkbox_x) {
                        first.task = Some(item.task);
                        first.task_checkbox_x = Some(checkbox_x);
                    }
                    y = append_lines(output, lines, y);
                    y = y.saturating_add(self.spacing_after(child));
                    first_child = Some(1);
                }
            } else {
                let mut spans = Vec::new();
                for inline in &item.content {
                    collect_spans(
                        inline,
                        self.style.body,
                        None,
                        image_height,
                        images,
                        &mut spans,
                    );
                }
                let mut lines = wrap_spans(
                    &self.measurer,
                    spans,
                    y,
                    text_x,
                    text_width,
                    self.style.body.line_height,
                );
                prepend_list_marker(
                    &mut lines,
                    &marker,
                    item_x,
                    marker_width,
                    self.style.body,
                    self.style.body.line_height,
                );
                if let (Some(first), Some(checkbox_x)) = (lines.first_mut(), task_checkbox_x) {
                    first.task = Some(item.task);
                    first.task_checkbox_x = Some(checkbox_x);
                }
                y = append_lines(output, lines, y);
                first_child = Some(0);
            }

            if first_child.is_none() {
                let mut line = marker_only_line(
                    item_x,
                    y,
                    &marker,
                    marker_width,
                    self.style.body,
                    self.style.body.line_height,
                );
                if let Some(checkbox_x) = task_checkbox_x {
                    line.task = Some(item.task);
                    line.task_checkbox_x = Some(checkbox_x);
                }
                output.push(line);
                y = y.saturating_add(self.style.body.line_height.max(1) as i32);
            }

            // Children retain their block semantics and can themselves contain
            // paragraphs, quotes, or another list. Their x origin advances once
            // per nesting level and past the marker column, so later lines stay
            // in the item's text column.
            if !tight && !item.content.is_empty() && !item.children.is_empty() {
                y = y.saturating_add(self.style.paragraph_spacing as i32);
            }
            for child in item.children.iter().skip(first_child.unwrap_or(0)) {
                y = y.saturating_add(self.spacing_before(child));
                let (_, _, lines, _) =
                    self.layout_content(child, y, text_x, text_width, image_height, images);
                y = append_lines(output, lines, y);
                y = y.saturating_add(self.list_child_spacing(tight, child));
            }
            y = y.saturating_add(if tight {
                self.style.tight_list_item_spacing as i32
            } else {
                self.style.list_item_spacing as i32
            });
        }

        y
    }

    fn list_child_spacing(&self, list_tight: bool, child: &Block) -> i32 {
        if list_tight && !matches!(child, Block::Heading { .. }) {
            0
        } else {
            self.spacing_after(child)
        }
    }

    fn quote_lines(
        &self,
        blocks: &[Block],
        start_y: i32,
        x: i32,
        width: u32,
        image_height: u32,
        images: &ImageResources,
    ) -> Vec<LayoutLine> {
        let indent = self
            .style
            .block_quote_indent
            .min(width)
            .saturating_add(self.style.block_quote_padding.min(width));
        let inner_x = x.saturating_add(indent as i32);
        let inner_width = width.saturating_sub(indent);
        let mut result = Vec::new();
        let mut y = start_y;

        for block in blocks {
            y = y.saturating_add(self.spacing_before(block));
            let (_, _, lines, _) =
                self.layout_content(block, y, inner_x, inner_width, image_height, images);
            y = append_lines(&mut result, lines, y);
            y = y.saturating_add(self.spacing_after(block));
        }

        if result.is_empty() {
            result.push(empty_line(inner_x, start_y, self.style.body.line_height));
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn alert_lines(
        &self,
        title: &str,
        blocks: &[Block],
        start_y: i32,
        x: i32,
        width: u32,
        image_height: u32,
        images: &ImageResources,
    ) -> Vec<LayoutLine> {
        let indent = self
            .style
            .block_quote_indent
            .min(width)
            .saturating_add(self.style.block_quote_padding.min(width));
        let inner_x = x.saturating_add(indent as i32);
        let inner_width = width.saturating_sub(indent);
        let title_style = TextStyle {
            bold: true,
            ..self.style.body
        };
        let mut result = wrap_spans(
            &self.measurer,
            vec![Span::new(format!("{title}: "), title_style, None, false)],
            start_y,
            inner_x,
            inner_width,
            title_style.line_height,
        );
        let mut y = result
            .last()
            .map(|line| rect_bottom(&line.bounds))
            .unwrap_or(start_y);

        for block in blocks {
            y = y.saturating_add(self.spacing_before(block));
            let (_, _, lines, _) =
                self.layout_content(block, y, inner_x, inner_width, image_height, images);
            y = append_lines(&mut result, lines, y);
            y = y.saturating_add(self.spacing_after(block));
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn footnote_lines(
        &self,
        name: &str,
        blocks: &[Block],
        start_y: i32,
        x: i32,
        width: u32,
        image_height: u32,
        images: &ImageResources,
    ) -> Vec<LayoutLine> {
        let prefix = Inline::Text(format!("[^{name}]: "));
        let (first, rest) = match blocks.split_first() {
            Some((Block::Paragraph(content), rest)) => {
                let mut inlines = vec![prefix.clone()];
                inlines.extend(content.iter().cloned());
                (Some(inlines), rest)
            }
            _ => (None, blocks),
        };
        let mut result = first
            .map(|content| {
                self.inline_lines_with_images(
                    &content,
                    self.style.body,
                    start_y,
                    x,
                    width,
                    image_height,
                    images,
                )
            })
            .unwrap_or_else(|| {
                self.inline_lines_with_images(
                    &[prefix],
                    self.style.body,
                    start_y,
                    x,
                    width,
                    image_height,
                    images,
                )
            });
        let mut y = result
            .last()
            .map(|line| rect_bottom(&line.bounds))
            .unwrap_or(start_y);
        for block in rest {
            y = y.saturating_add(self.spacing_before(block));
            let (_, _, lines, _) = self.layout_content(block, y, x, width, image_height, images);
            y = append_lines(&mut result, lines, y);
            y = y.saturating_add(self.spacing_after(block));
        }
        result
    }

    fn table_lines(
        &self,
        table: &Table,
        start_y: i32,
        x: i32,
        width: u32,
        image_height: u32,
        images: &ImageResources,
    ) -> (Vec<LayoutLine>, TableLayout) {
        let column_count = table
            .headers
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        if column_count == 0 {
            return (
                vec![empty_line(x, start_y, self.style.body.line_height)],
                TableLayout {
                    mode: TableLayoutMode::Normal,
                    columns: Vec::new(),
                    groups: Vec::new(),
                    rows: Vec::new(),
                },
            );
        }

        let all_columns = (0..column_count).collect::<Vec<_>>();
        let normal = self.table_text_style(false, false);
        let compact = self.table_text_style(true, false);
        let aggressive = self.table_text_style(true, true);
        // Fit ordinary tables at body typography first. Compact and
        // aggressive modes are fallbacks for tables that need more space.
        let stages = vec![
            (TableLayoutMode::Normal, normal),
            (TableLayoutMode::Compact, compact),
            (TableLayoutMode::Aggressive, aggressive),
        ];

        for (mode, text_style) in stages {
            let columns = self.table_columns(table, column_count, text_style, image_height, images);
            let Some(widths) = self
                .allocate_table_widths(
                    &all_columns,
                    &columns,
                    width,
                    mode == TableLayoutMode::Aggressive,
                )
                .filter(|widths| self.table_widths_are_readable(widths, text_style))
            else {
                continue;
            };

            if self.table_widths_are_useful(&columns, &widths, width) {
                let groups = vec![TableColumnGroup {
                    columns: all_columns,
                    width: table_group_width(
                        &widths,
                        self.style.table_cell_padding,
                        self.style.table_border.width.max(1),
                    ),
                    alignments: table
                        .alignments
                        .iter()
                        .copied()
                        .chain(std::iter::repeat(crate::document::TableAlignment::None))
                        .take(column_count)
                        .collect(),
                    x,
                    widths,
                }];
                return self.render_table(
                    table,
                    start_y,
                    text_style,
                    mode,
                    columns,
                    groups,
                    image_height,
                    images,
                );
            }

            // A readable minimum is not sufficient when one prose column is
            // compressed to a few words per line. Continue the table in
            // preferred-width groups before trying a smaller font.
            return self.render_grouped_table(
                table,
                start_y,
                width,
                x,
                text_style,
                columns,
                false,
                image_height,
                images,
            );
        }

        // Even character-level wrapping cannot make a table with more cells
        // than the physical viewport can frame fit horizontally. Continue it
        // as deterministic vertical groups. The first source column is kept in
        // every group where the viewport can hold it; this keeps row identity
        // visible while reading a continuation group.
        let text_style = self.table_text_style(true, true);
        let columns = self.table_columns(table, column_count, text_style, image_height, images);
        self.render_grouped_table(
            table,
            start_y,
            width,
            x,
            text_style,
            columns,
            true,
            image_height,
            images,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_grouped_table(
        &self,
        table: &Table,
        start_y: i32,
        width: u32,
        x: i32,
        text_style: TextStyle,
        columns: Vec<TableColumnLayout>,
        aggressive_minimums: bool,
        image_height: u32,
        images: &ImageResources,
    ) -> (Vec<LayoutLine>, TableLayout) {
        let column_count = columns.len();
        let mut groups = Vec::new();
        let mut pending = vec![0];
        for index in 1..column_count {
            let mut candidate = pending.clone();
            candidate.push(index);
            let fits_preferred_budget = self.group_preferred_width(&candidate, &columns, width)
                <= width.saturating_sub(table_group_overhead(
                    candidate.len(),
                    self.style.table_cell_padding,
                    self.style.table_border.width.max(1),
                ));
            let fits_minimum_width = self
                .allocate_table_widths(&candidate, &columns, width, aggressive_minimums)
                .filter(|widths| self.table_widths_are_readable(widths, text_style))
                .is_some();
            if fits_preferred_budget && fits_minimum_width {
                pending = candidate;
            } else {
                groups.push(self.make_table_group(
                    &pending,
                    &columns,
                    width,
                    x,
                    text_style,
                    aggressive_minimums,
                ));
                pending = vec![0, index];
                if self
                    .allocate_table_widths(&pending, &columns, width, aggressive_minimums)
                    .filter(|widths| self.table_widths_are_readable(widths, text_style))
                    .is_none()
                {
                    // At an exceptionally narrow viewport, preserve progress
                    // even when duplicating the key column is impossible.
                    groups.push(self.make_table_group(
                        &[index],
                        &columns,
                        width,
                        x,
                        text_style,
                        true,
                    ));
                    pending = vec![0];
                }
            }
        }
        if pending.len() > 1 || groups.is_empty() {
            groups.push(self.make_table_group(
                &pending,
                &columns,
                width,
                x,
                text_style,
                aggressive_minimums,
            ));
        }

        self.render_table(
            table,
            start_y,
            text_style,
            TableLayoutMode::Grouped,
            columns,
            groups,
            image_height,
            images,
        )
    }

    fn table_text_style(&self, compact: bool, aggressive: bool) -> TextStyle {
        if !compact {
            return self.style.body;
        }
        let minimum = self
            .style
            .table_min_font_size
            .min(self.style.body.font_size.max(1));
        let target_size = if aggressive {
            self.style.body.font_size.saturating_mul(2) / 3
        } else {
            self.style.body.font_size.saturating_mul(3) / 4
        };
        let font_size = target_size.max(minimum).max(1);
        TextStyle {
            font_size,
            line_height: self
                .style
                .body
                .line_height
                .saturating_mul(font_size)
                .checked_div(self.style.body.font_size.max(1))
                .unwrap_or(font_size)
                .max(font_size)
                .max(self.style.table_min_line_height.max(1)),
            ..self.style.body
        }
    }

    fn table_columns(
        &self,
        table: &Table,
        column_count: usize,
        text_style: TextStyle,
        image_height: u32,
        images: &ImageResources,
    ) -> Vec<TableColumnLayout> {
        (0..column_count)
            .map(|index| {
                let mut minimum_width = 0;
                let mut preferred_width = 0;
                let mut inspect = |cell: &[Inline], cell_style: TextStyle| {
                    let (minimum, preferred) =
                        self.table_cell_metrics(cell, cell_style, image_height, images);
                    minimum_width = minimum_width.max(minimum);
                    preferred_width = preferred_width.max(preferred);
                };
                if let Some(cell) = table.headers.get(index) {
                    inspect(
                        cell,
                        TextStyle {
                            bold: true,
                            ..text_style
                        },
                    );
                }
                for row in &table.rows {
                    if let Some(cell) = row.get(index) {
                        inspect(cell, text_style);
                    }
                }
                TableColumnLayout {
                    index,
                    minimum_width,
                    preferred_width: preferred_width.max(minimum_width),
                    alignment: table.alignments.get(index).copied().unwrap_or_default(),
                }
            })
            .collect()
    }

    fn table_cell_metrics(
        &self,
        inlines: &[Inline],
        style: TextStyle,
        image_height: u32,
        images: &ImageResources,
    ) -> (u32, u32) {
        let mut spans = Vec::new();
        for inline in inlines {
            collect_spans(inline, style, None, image_height, images, &mut spans);
        }
        let lines = wrap_spans(
            &self.measurer,
            spans.clone(),
            0,
            0,
            u32::MAX / 4,
            style.line_height,
        );
        let preferred = lines
            .iter()
            .map(|line| line.bounds.size.width)
            .max()
            .unwrap_or(0);
        // `append_token` splits an over-wide word at character boundaries.
        // Keep short tokens intact for useful column widths, but cap a long
        // token before it can force compact typography. The capped remainder
        // uses the same character-wrap fallback as cell layout.
        let token_limit = style.font_size.saturating_mul(5).max(1);
        let mut minimum = 0;
        for span in &spans {
            for token in span.text.split(char::is_whitespace) {
                let token_width = self.measurer.measure(token, &span.style);
                let character_width = token
                    .chars()
                    .map(|character| self.measurer.measure(&character.to_string(), &span.style))
                    .max()
                    .unwrap_or(0);
                minimum = minimum.max(token_width.min(token_limit).max(character_width));
            }
        }
        (minimum, preferred.max(minimum))
    }

    fn allocate_table_widths(
        &self,
        indexes: &[usize],
        columns: &[TableColumnLayout],
        table_width: u32,
        aggressive: bool,
    ) -> Option<Vec<u32>> {
        let overhead = table_group_overhead(
            indexes.len(),
            self.style.table_cell_padding,
            self.style.table_border.width.max(1),
        );
        let available = table_width.checked_sub(overhead)?;
        if available < indexes.len() as u32 {
            return None;
        }
        let mut minimums = indexes
            .iter()
            .map(|index| {
                if aggressive {
                    self.style
                        .body
                        .font_size
                        .max(self.style.table_min_font_size)
                        .saturating_add(8)
                } else {
                    columns[*index].minimum_width.max(1)
                }
            })
            .collect::<Vec<_>>();
        let preferreds = indexes
            .iter()
            .zip(minimums.iter())
            .map(|(index, minimum)| columns[*index].preferred_width.max(*minimum))
            .collect::<Vec<_>>();
        let minimum_total = minimums.iter().copied().sum::<u32>();
        if minimum_total > available {
            if !aggressive {
                return None;
            }
            // Preserve the old last-resort progress guarantee when even the
            // readable floor cannot fit. The grouped fallback still keeps
            // each column framed and repeats the key where possible.
            minimums.fill(1);
        }

        let minimum_total = minimums.iter().copied().sum::<u32>();
        if minimum_total > available {
            return None;
        }

        let mut widths = minimums;
        let mut remaining = available - minimum_total;

        // Grow toward preferred content widths in proportion to each column's
        // remaining demand. This reserves every usable minimum first and
        // prevents source order from consuming spare width before a later
        // prose or detail column can use it.
        while remaining > 0 {
            let deficits = widths
                .iter()
                .zip(preferreds.iter())
                .map(|(width, preferred)| preferred.saturating_sub(*width))
                .collect::<Vec<_>>();
            let total_deficit = deficits.iter().copied().sum::<u32>();
            if total_deficit == 0 {
                break;
            }

            let mut distributed: u32 = 0;
            for (index, deficit) in deficits.iter().copied().enumerate() {
                if deficit == 0 {
                    continue;
                }
                let growth = ((remaining as u64 * deficit as u64) / total_deficit as u64)
                    .min(deficit as u64) as u32;
                widths[index] = widths[index].saturating_add(growth);
                distributed = distributed.saturating_add(growth);
            }
            if distributed == 0 {
                let index = deficits
                    .iter()
                    .enumerate()
                    .filter(|(_, deficit)| **deficit > 0)
                    .max_by_key(|(index, deficit)| (**deficit, std::cmp::Reverse(*index)))
                    .map(|(index, _)| index)
                    .expect("a positive total deficit has a positive member");
                widths[index] = widths[index].saturating_add(1);
                distributed = 1;
            }
            remaining -= distributed;
        }
        // Any spare space after all preferred widths are met is distributed
        // one pixel at a time. This keeps the result deterministic.
        let mut index = 0;
        while remaining > 0 && !widths.is_empty() {
            widths[index] = widths[index].saturating_add(1);
            remaining -= 1;
            index = (index + 1) % widths.len();
        }
        Some(widths)
    }

    fn table_widths_are_readable(&self, widths: &[u32], text_style: TextStyle) -> bool {
        widths.len() <= 1
            || widths.iter().all(|width| {
                *width
                    >= text_style
                        .font_size
                        .saturating_add(8)
                        .max(self.style.table_min_font_size.saturating_add(8))
                        .max(12)
            })
    }

    fn table_widths_are_useful(
        &self,
        columns: &[TableColumnLayout],
        widths: &[u32],
        table_width: u32,
    ) -> bool {
        if columns.len() < 6 {
            return true;
        }
        let available = table_width.saturating_sub(table_group_overhead(
            columns.len(),
            self.style.table_cell_padding,
            self.style.table_border.width.max(1),
        ));
        let preferred_total = columns
            .iter()
            .map(|column| column.preferred_width)
            .fold(0, u32::saturating_add);
        let has_severely_compressed_column = columns
            .iter()
            .zip(widths.iter())
            .any(|(column, width)| column.preferred_width > width.saturating_mul(3));
        let has_excess_preferred_demand = preferred_total > available.saturating_mul(2);
        !(has_excess_preferred_demand || has_severely_compressed_column)
    }

    fn group_preferred_width(
        &self,
        indexes: &[usize],
        columns: &[TableColumnLayout],
        table_width: u32,
    ) -> u32 {
        let available = table_width.saturating_sub(table_group_overhead(
            indexes.len(),
            self.style.table_cell_padding,
            self.style.table_border.width.max(1),
        ));
        let key_width = indexes
            .iter()
            .find(|index| **index == 0)
            .map(|index| columns[*index].minimum_width.max(1).min(available))
            .unwrap_or(0);
        indexes
            .iter()
            .map(|index| {
                let preferred = columns[*index].preferred_width.max(1);
                if *index == 0 {
                    preferred.min(available)
                } else {
                    preferred.min(available.saturating_sub(key_width))
                }
            })
            .fold(0, u32::saturating_add)
    }

    fn make_table_group(
        &self,
        indexes: &[usize],
        columns: &[TableColumnLayout],
        width: u32,
        x: i32,
        text_style: TextStyle,
        aggressive: bool,
    ) -> TableColumnGroup {
        let widths = self
            .allocate_table_widths(indexes, columns, width, aggressive)
            .filter(|widths| self.table_widths_are_readable(widths, text_style))
            .unwrap_or_else(|| vec![1; indexes.len()]);
        TableColumnGroup {
            columns: indexes.to_vec(),
            width: table_group_width(
                &widths,
                self.style.table_cell_padding,
                self.style.table_border.width.max(1),
            ),
            alignments: indexes
                .iter()
                .map(|index| columns[*index].alignment)
                .collect(),
            x,
            widths,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_table(
        &self,
        table: &Table,
        start_y: i32,
        text_style: TextStyle,
        mode: TableLayoutMode,
        columns: Vec<TableColumnLayout>,
        groups: Vec<TableColumnGroup>,
        image_height: u32,
        images: &ImageResources,
    ) -> (Vec<LayoutLine>, TableLayout) {
        let mut lines = Vec::new();
        let mut rows = Vec::new();
        let mut y = start_y;
        for (group_index, group) in groups.iter().enumerate() {
            let header = if table.headers.is_empty() {
                None
            } else {
                Some(table.headers.as_slice())
            };
            if let Some(header) = header {
                y = self.render_table_row(
                    header,
                    true,
                    group_index,
                    group,
                    text_style,
                    y,
                    &mut lines,
                    &mut rows,
                    image_height,
                    images,
                );
            }
            for source_row in &table.rows {
                y = self.render_table_row(
                    source_row,
                    false,
                    group_index,
                    group,
                    text_style,
                    y,
                    &mut lines,
                    &mut rows,
                    image_height,
                    images,
                );
            }
        }
        if lines.is_empty() {
            lines.push(empty_line(
                groups.first().map(|group| group.x).unwrap_or(0),
                start_y,
                text_style.line_height,
            ));
        }
        (
            lines,
            TableLayout {
                mode,
                columns,
                groups,
                rows,
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_table_row(
        &self,
        source_row: &[Vec<Inline>],
        header: bool,
        group_index: usize,
        group: &TableColumnGroup,
        text_style: TextStyle,
        start_y: i32,
        output: &mut Vec<LayoutLine>,
        rows: &mut Vec<TableRowLayout>,
        image_height: u32,
        images: &ImageResources,
    ) -> i32 {
        let cell_padding = self.style.table_cell_padding;
        let vertical_padding = self.style.table_cell_vertical_padding;
        let cell_lines = group
            .columns
            .iter()
            .zip(group.widths.iter())
            .scan(
                group.x + self.style.table_border.width.max(1) as i32,
                |cell_x, (column, column_width)| {
                    let inlines = source_row.get(*column).map(Vec::as_slice).unwrap_or(&[]);
                    let cell_style = TextStyle {
                        bold: header,
                        ..text_style
                    };
                    let mut lines = self.inline_lines_with_images(
                        inlines,
                        cell_style,
                        start_y.saturating_add(vertical_padding as i32),
                        cell_x.saturating_add(cell_padding as i32),
                        (*column_width).max(1),
                        image_height,
                        images,
                    );
                    align_cell_lines(
                        &mut lines,
                        *column_width,
                        group
                            .columns
                            .iter()
                            .position(|index| index == column)
                            .and_then(|index| group.alignments.get(index).copied())
                            .unwrap_or_default(),
                    );
                    *cell_x = cell_x.saturating_add(
                        (*column_width
                            + cell_padding.saturating_mul(2)
                            + self.style.table_border.width.max(1)) as i32,
                    );
                    Some(lines)
                },
            )
            .collect::<Vec<_>>();
        let row_height = cell_lines.iter().map(Vec::len).max().unwrap_or(1);
        let line_heights = (0..row_height)
            .map(|line_index| {
                cell_lines
                    .iter()
                    .filter_map(|lines| lines.get(line_index))
                    .map(|line| line.bounds.size.height)
                    .max()
                    .unwrap_or(text_style.line_height.max(1))
            })
            .collect::<Vec<_>>();
        let row_start = output.len();
        let mut content_y = start_y.saturating_add(vertical_padding as i32);
        let mut line_y = start_y;
        for (line_index, height) in line_heights.iter().copied().enumerate() {
            let mut fragments = Vec::new();
            for lines in &cell_lines {
                if let Some(line) = lines.get(line_index) {
                    let y_offset = content_y.saturating_sub(line.bounds.top_left.y);
                    fragments.extend(line.fragments.iter().cloned().map(|mut fragment| {
                        fragment.bounds = Rectangle::new(
                            Point::new(
                                fragment.bounds.top_left.x,
                                fragment.bounds.top_left.y.saturating_add(y_offset),
                            ),
                            fragment.bounds.size,
                        );
                        fragment
                    }));
                }
            }
            let top_padding = if line_index == 0 { vertical_padding } else { 0 };
            let bottom_padding = if line_index + 1 == row_height {
                vertical_padding
            } else {
                0
            };
            output.push(LayoutLine {
                bounds: Rectangle::new(
                    Point::new(group.x, line_y),
                    Size::new(
                        group.width,
                        height
                            .saturating_add(top_padding)
                            .saturating_add(bottom_padding),
                    ),
                ),
                fragments,
                task: None,
                task_checkbox_x: None,
                code: false,
                wrapped: false,
            });
            content_y = content_y.saturating_add(height as i32);
            line_y = content_y;
        }
        rows.push(TableRowLayout {
            line_range: row_start..output.len(),
            group: group_index,
            header,
        });
        content_y.saturating_add(vertical_padding as i32)
    }

    fn code_lines(
        &self,
        language: Option<&str>,
        code: &str,
        start_y: i32,
        x: i32,
        width: u32,
    ) -> Vec<LayoutLine> {
        let surface_width = width.max(1);
        // Keep at least one pixel for code text on narrow diagnostic
        // viewports. The normal T1 content width leaves the configured inset
        // on both sides.
        let padding = self
            .style
            .code_block_padding
            .min(surface_width.saturating_sub(1) / 2);
        let text_x = x.saturating_add(padding as i32);
        let text_width = surface_width
            .saturating_sub(padding.saturating_mul(2))
            .max(1);
        let highlighted = self.code_highlighter.highlight(language, code);
        let mut result = Vec::new();
        let mut y = start_y;
        for source_line in highlighted.lines {
            let spans = source_line
                .spans
                .into_iter()
                .map(|span| {
                    Span::new(
                        span.text,
                        TextStyle {
                            code: true,
                            bold: self.style.code.bold || span.bold,
                            italic: self.style.code.italic || span.italic,
                            ink: span.ink,
                            ..self.style.code
                        },
                        None,
                        true,
                    )
                })
                .collect();
            let mut lines = wrap_spans(
                &self.measurer,
                spans,
                y,
                text_x,
                text_width,
                self.style.code.line_height,
            );
            for (line_index, line) in lines.iter_mut().enumerate() {
                line.code = true;
                line.wrapped = line_index > 0;
                // `LineBuilder` uses used text width for ordinary prose.
                // Code lines instead expose the stable block surface to
                // pagination; fragments keep their inset text bounds.
                line.bounds = Rectangle::new(
                    Point::new(x, line.bounds.top_left.y),
                    Size::new(surface_width, line.bounds.size.height),
                );
            }
            if let Some(last) = lines.last() {
                y = last
                    .bounds
                    .top_left
                    .y
                    .saturating_add(last.bounds.size.height as i32);
            }
            result.extend(lines);
        }
        if result.is_empty() {
            result.push(LayoutLine {
                bounds: Rectangle::new(
                    Point::new(x, start_y),
                    Size::new(surface_width, self.style.code.line_height.max(1)),
                ),
                fragments: Vec::new(),
                task: None,
                task_checkbox_x: None,
                code: true,
                wrapped: false,
            });
        }
        result
    }
}

fn table_group_overhead(column_count: usize, padding: u32, border_width: u32) -> u32 {
    (column_count as u32)
        .saturating_mul(padding.saturating_mul(2))
        .saturating_add((column_count.saturating_add(1) as u32).saturating_mul(border_width.max(1)))
}

fn table_group_width(widths: &[u32], padding: u32, border_width: u32) -> u32 {
    table_group_overhead(widths.len(), padding, border_width)
        .saturating_add(widths.iter().copied().sum::<u32>())
}

fn align_cell_lines(
    lines: &mut [LayoutLine],
    cell_width: u32,
    alignment: crate::document::TableAlignment,
) {
    for line in lines {
        let spare = cell_width.saturating_sub(line.bounds.size.width);
        let offset = match alignment {
            crate::document::TableAlignment::Center => spare / 2,
            crate::document::TableAlignment::Right => spare,
            crate::document::TableAlignment::None | crate::document::TableAlignment::Left => 0,
        } as i32;
        if offset == 0 {
            continue;
        }
        line.bounds = Rectangle::new(
            Point::new(
                line.bounds.top_left.x.saturating_add(offset),
                line.bounds.top_left.y,
            ),
            line.bounds.size,
        );
        for fragment in &mut line.fragments {
            fragment.bounds = Rectangle::new(
                Point::new(
                    fragment.bounds.top_left.x.saturating_add(offset),
                    fragment.bounds.top_left.y,
                ),
                fragment.bounds.size,
            );
        }
    }
}

#[derive(Clone, Debug)]
struct Span {
    text: String,
    style: TextStyle,
    link: Option<NavigationTarget>,
    preserve_whitespace: bool,
    image: Option<LayoutImage>,
}

impl Span {
    fn new(
        text: impl Into<String>,
        style: TextStyle,
        link: Option<NavigationTarget>,
        preserve_whitespace: bool,
    ) -> Self {
        Self {
            text: text.into(),
            style,
            link,
            preserve_whitespace,
            image: None,
        }
    }

    fn image(
        image: RasterImage,
        source: impl Into<String>,
        alt: impl Into<String>,
        style: TextStyle,
        link: Option<NavigationTarget>,
    ) -> Self {
        Self {
            text: String::new(),
            style,
            link,
            preserve_whitespace: false,
            image: Some(LayoutImage::new(image, source, alt)),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_spans(
    inline: &Inline,
    style: TextStyle,
    inherited_link: Option<NavigationTarget>,
    image_height: u32,
    images: &ImageResources,
    spans: &mut Vec<Span>,
) {
    match inline {
        Inline::Text(text) => spans.push(Span::new(text, style, inherited_link, false)),
        Inline::Code(text) => spans.push(Span::new(
            text,
            TextStyle {
                code: true,
                ..style
            },
            inherited_link,
            true,
        )),
        Inline::Emphasis(children) => {
            for child in children {
                collect_spans(
                    child,
                    TextStyle {
                        italic: true,
                        ..style
                    },
                    inherited_link.clone(),
                    image_height,
                    images,
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
                    image_height,
                    images,
                    spans,
                );
            }
        }
        Inline::Strikethrough(children) => {
            for child in children {
                collect_spans(
                    child,
                    TextStyle {
                        strikethrough: true,
                        ..style
                    },
                    inherited_link.clone(),
                    image_height,
                    images,
                    spans,
                );
            }
        }
        Inline::Link {
            label, destination, ..
        } => {
            let target = NavigationTarget::from_destination(destination);
            for child in label {
                collect_spans(
                    child,
                    TextStyle {
                        underline: true,
                        ..style
                    },
                    Some(target.clone()),
                    image_height,
                    images,
                    spans,
                );
            }
        }
        Inline::Image { alt, source, .. } => {
            if let Some(image) = images.image(source) {
                spans.push(Span::image(
                    image.fitted(u32::MAX, image_height),
                    source,
                    alt,
                    style,
                    inherited_link,
                ));
            } else {
                spans.push(Span::new(image_fallback(alt), style, inherited_link, false));
            }
        }
        Inline::FootnoteReference { name, number } => spans.push(Span::new(
            if *number == 0 {
                format!("[^{name}]")
            } else {
                format!("[{number}]")
            },
            style,
            inherited_link,
            false,
        )),
        // A soft break is whitespace in Markdown and may wrap naturally. A
        // hard break is an explicit legal line boundary.
        Inline::SoftBreak => spans.push(Span::new(" ", style, inherited_link, false)),
        Inline::HardBreak => spans.push(Span::new("\n", style, inherited_link, false)),
    }
}

fn list_marker(ordered: bool, start: usize, index: usize) -> String {
    let ordinal = start.saturating_add(index);
    if ordered {
        format!("{ordinal}. ")
    } else {
        "• ".to_owned()
    }
}

fn prepend_list_marker(
    lines: &mut [LayoutLine],
    marker: &str,
    item_x: i32,
    marker_width: u32,
    marker_style: TextStyle,
    line_height: u32,
) {
    let Some(first_line) = lines.first_mut() else {
        return;
    };
    let right = rect_right(&first_line.bounds);
    first_line.bounds = Rectangle::new(
        Point::new(item_x, first_line.bounds.top_left.y),
        Size::new(
            right.saturating_sub(item_x).max(marker_width as i32) as u32,
            first_line.bounds.size.height.max(line_height.max(1)),
        ),
    );
    if !marker.is_empty() {
        first_line.fragments.insert(
            0,
            LayoutFragment {
                text: marker.to_owned(),
                bounds: Rectangle::new(
                    Point::new(item_x, first_line.bounds.top_left.y),
                    Size::new(marker_width, line_height.max(1)),
                ),
                style: marker_style,
                image: None,
                link: None,
            },
        );
    }
}

fn marker_only_line(
    item_x: i32,
    y: i32,
    marker: &str,
    marker_width: u32,
    marker_style: TextStyle,
    line_height: u32,
) -> LayoutLine {
    LayoutLine {
        bounds: Rectangle::new(
            Point::new(item_x, y),
            Size::new(marker_width, line_height.max(1)),
        ),
        fragments: if marker.is_empty() {
            Vec::new()
        } else {
            vec![LayoutFragment {
                text: marker.to_owned(),
                bounds: Rectangle::new(
                    Point::new(item_x, y),
                    Size::new(marker_width, line_height.max(1)),
                ),
                style: marker_style,
                image: None,
                link: None,
            }]
        },
        task: None,
        task_checkbox_x: None,
        code: false,
        wrapped: false,
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
    fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width: width.max(1),
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
        let actual_width = measured_width.min(available);
        self.fragments.push(LayoutFragment {
            text,
            bounds: Rectangle::new(
                Point::new(self.x.saturating_add(self.used as i32), self.y),
                Size::new(actual_width, style.line_height.max(1)),
            ),
            style,
            image: None,
            link,
        });
        self.used = self.used.saturating_add(actual_width);
        self.height = self.height.max(style.line_height.max(1));
    }

    fn add_image(&mut self, image: LayoutImage, style: TextStyle, link: Option<NavigationTarget>) {
        let width = image
            .image
            .width()
            .min(self.width.saturating_sub(self.used));
        let bounds = Rectangle::new(
            Point::new(self.x.saturating_add(self.used as i32), self.y),
            Size::new(width, image.image.height()),
        );
        self.fragments.push(LayoutFragment {
            text: String::new(),
            bounds,
            style,
            image: Some(image),
            link,
        });
        self.used = self.used.saturating_add(width);
        self.height = self.height.max(bounds.size.height.max(style.line_height));
    }

    fn finish(self) -> LayoutLine {
        LayoutLine {
            bounds: Rectangle::new(
                Point::new(self.x, self.y),
                Size::new(self.used, self.height),
            ),
            fragments: self.fragments,
            task: None,
            task_checkbox_x: None,
            code: false,
            wrapped: false,
        }
    }
}

fn wrap_spans<M: TextMeasurer>(
    measurer: &M,
    spans: Vec<Span>,
    start_y: i32,
    x: i32,
    width: u32,
    line_height: u32,
) -> Vec<LayoutLine> {
    // A zero-width content box can occur when margins exceed a tiny host
    // viewport. Use one internal pixel for progress; pagination clips output.
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = LineBuilder::new(x, start_y, width, line_height);
    let mut pending_space: Option<(TextStyle, Option<NavigationTarget>)> = None;

    for span in spans {
        if let Some(image) = span.image {
            append_image(
                &mut line,
                &mut lines,
                &mut pending_space,
                image,
                span.style,
                span.link,
            );
            continue;
        }
        let mut word = String::new();
        for character in span.text.chars() {
            if character == '\n' {
                append_word(
                    measurer,
                    &mut line,
                    &mut lines,
                    &mut pending_space,
                    &word,
                    &span,
                );
                line = finish_line(line, &mut lines);
                word.clear();
                pending_space = None;
            } else if character.is_whitespace() {
                append_word(
                    measurer,
                    &mut line,
                    &mut lines,
                    &mut pending_space,
                    &word,
                    &span,
                );
                word.clear();
                if span.preserve_whitespace {
                    append_token(
                        measurer,
                        &mut line,
                        &mut lines,
                        &character.to_string(),
                        span.style,
                        span.link.clone(),
                        true,
                    );
                } else if line.has_content() {
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
                        false,
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
            &word,
            &span,
        );
    }

    if line.has_content() || lines.is_empty() {
        lines.push(line.finish());
    }
    lines
}

fn append_image(
    line: &mut LineBuilder,
    lines: &mut Vec<LayoutLine>,
    pending_space: &mut Option<(TextStyle, Option<NavigationTarget>)>,
    image: LayoutImage,
    style: TextStyle,
    link: Option<NavigationTarget>,
) {
    pending_space.take();
    let image = LayoutImage::new(
        image.image.fitted(line.width, u32::MAX),
        image.source,
        image.alt,
    );
    let width = image.image.width();
    if line.has_content() && line.used.saturating_add(width) > line.width {
        let old = std::mem::replace(
            line,
            LineBuilder::new(
                line.x,
                line.y + line.height as i32,
                line.width,
                image.image.height().max(style.line_height),
            ),
        );
        lines.push(old.finish());
    }
    line.add_image(image, style, link);
}

fn append_word<M: TextMeasurer>(
    measurer: &M,
    line: &mut LineBuilder,
    lines: &mut Vec<LayoutLine>,
    pending_space: &mut Option<(TextStyle, Option<NavigationTarget>)>,
    word: &str,
    span: &Span,
) {
    if word.is_empty() {
        return;
    }
    if let Some((space_style, space_link)) = pending_space.take() {
        append_token(measurer, line, lines, " ", space_style, space_link, false);
    }
    append_token(
        measurer,
        line,
        lines,
        word,
        span.style,
        span.link.clone(),
        false,
    );
}

fn append_token<M: TextMeasurer>(
    measurer: &M,
    line: &mut LineBuilder,
    lines: &mut Vec<LayoutLine>,
    text: &str,
    style: TextStyle,
    link: Option<NavigationTarget>,
    allow_leading_space: bool,
) {
    if text.is_empty() || (text == " " && !line.has_content() && !allow_leading_space) {
        return;
    }
    let measured = measurer.measure(text, &style);
    let width = line.width;
    if line.has_content() && line.used.saturating_add(measured) > width {
        let old = std::mem::replace(
            line,
            LineBuilder::new(
                line.x,
                line.y + line.height as i32,
                width,
                style.line_height,
            ),
        );
        lines.push(old.finish());
        if text == " " && !allow_leading_space {
            return;
        }
    }

    if measured <= width || text.chars().count() <= 1 {
        line.add(text.to_owned(), measured.min(width), style, link);
        return;
    }

    // A single unbreakable word (or a long URL) is split at character
    // boundaries. This is the fallback legal split when no whitespace exists.
    for character in text.chars() {
        let character_text = character.to_string();
        let character_width = measurer.measure(&character_text, &style).min(width);
        if line.has_content() && line.used.saturating_add(character_width) > width {
            let old = std::mem::replace(
                line,
                LineBuilder::new(
                    line.x,
                    line.y + line.height as i32,
                    width,
                    style.line_height,
                ),
            );
            lines.push(old.finish());
        }
        line.add(character_text, character_width, style, link.clone());
    }
}

fn finish_line(line: LineBuilder, lines: &mut Vec<LayoutLine>) -> LineBuilder {
    let next_y = line.y.saturating_add(line.height as i32);
    let x = line.x;
    let width = line.width;
    lines.push(line.finish());
    LineBuilder::new(x, next_y, width, 1)
}

fn append_lines(output: &mut Vec<LayoutLine>, lines: Vec<LayoutLine>, fallback_y: i32) -> i32 {
    let bottom = lines
        .last()
        .map(|line| rect_bottom(&line.bounds))
        .unwrap_or(fallback_y);
    output.extend(lines);
    bottom
}

fn empty_line(x: i32, y: i32, line_height: u32) -> LayoutLine {
    LayoutLine {
        bounds: Rectangle::new(Point::new(x, y), Size::new(0, line_height.max(1))),
        fragments: Vec::new(),
        task: None,
        task_checkbox_x: None,
        code: false,
        wrapped: false,
    }
}

fn block_bounds(lines: &[LayoutLine], x: i32, y: i32) -> Rectangle {
    let right = lines
        .iter()
        .map(|line| rect_right(&line.bounds))
        .max()
        .unwrap_or(x);
    let bottom = lines
        .last()
        .map(|line| rect_bottom(&line.bounds))
        .unwrap_or(y);
    Rectangle::new(
        Point::new(x, y),
        Size::new(
            right.saturating_sub(x).max(0) as u32,
            bottom.saturating_sub(y).max(0) as u32,
        ),
    )
}

fn rect_right(rectangle: &Rectangle) -> i32 {
    rectangle
        .top_left
        .x
        .saturating_add(rectangle.size.width.min(i32::MAX as u32) as i32)
}

fn rect_bottom(rectangle: &Rectangle) -> i32 {
    rectangle
        .top_left
        .y
        .saturating_add(rectangle.size.height.min(i32::MAX as u32) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, Inline, ListItem};
    use crate::navigation::{DocumentId, NavigationTarget};
    use crate::pagination::{DisplayCommand, Paginator};
    use crate::style::Insets;

    fn style() -> ReaderStyle {
        ReaderStyle {
            page_padding: Insets::all(2),
            body: TextStyle::new(10, 12),
            heading: TextStyle {
                bold: true,
                ..TextStyle::new(20, 24)
            },
            paragraph_spacing: 5,
            heading_spacing_before: 7,
            heading_spacing_after: 3,
            list_indent: 12,
            block_quote_indent: 8,
            block_quote_padding: 2,
            ..ReaderStyle::default()
        }
    }

    fn all_text(lines: &[LayoutLine]) -> String {
        lines
            .iter()
            .flat_map(|line| line.fragments.iter().map(|fragment| fragment.text.as_str()))
            .collect()
    }

    #[test]
    fn width_sensitive_layout_wraps_without_exceeding_content_box() {
        let document = Document::from_blocks(vec![Block::paragraph(
            "A deliberately long paragraph with several legal split points.",
        )]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(58, 500));
        let block = &layout.blocks()[0];
        assert!(block.lines.len() > 2);
        assert!(block.lines.iter().all(|line| line.bounds.size.width <= 54));
        assert!(block
            .lines
            .windows(2)
            .all(|lines| lines[1].bounds.top_left.y >= lines[0].bounds.top_left.y));
    }

    #[test]
    fn soft_break_is_space_but_hard_break_is_a_new_line() {
        let document = Document::from_blocks(vec![Block::Paragraph(vec![
            Inline::Text("soft".into()),
            Inline::SoftBreak,
            Inline::Text("break".into()),
            Inline::HardBreak,
            Inline::Text("hard".into()),
        ])]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(300, 200));
        let lines = &layout.blocks()[0].lines;
        assert_eq!(lines.len(), 2);
        assert_eq!(all_text(lines), "soft breakhard");
    }

    #[test]
    fn inline_styles_and_code_preserve_semantics_in_fragments() {
        let document = Document::from_blocks(vec![Block::Paragraph(vec![
            Inline::Strong(vec![Inline::Text("bold".into())]),
            Inline::Emphasis(vec![Inline::Text(" italic".into())]),
            Inline::Strikethrough(vec![Inline::Text(" strike".into())]),
            Inline::Code(" x  y ".into()),
        ])]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(300, 200));
        let fragments = &layout.blocks()[0].lines[0].fragments;
        assert!(fragments.iter().any(|fragment| fragment.style.bold));
        assert!(fragments.iter().any(|fragment| fragment.style.italic));
        assert!(fragments
            .iter()
            .any(|fragment| fragment.style.strikethrough));
        assert!(fragments.iter().any(|fragment| fragment.style.code));
        assert!(fragments.iter().all(|fragment| !fragment.style.underline));
        assert!(fragments.iter().any(|fragment| fragment.text == " "));
    }

    #[test]
    fn fenced_code_uses_an_inset_text_box_and_stable_surface() {
        let style = ReaderStyle {
            code_block_padding: 4,
            ..style()
        };
        let document = Document::from_blocks(vec![Block::CodeBlock {
            language: None,
            info: None,
            code: "short\n\nThis deliberately long source line wraps inside the padded code surface and keeps every continuation aligned\nafter".into(),
        }]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(58, 500));
        let block = &layout.blocks()[0];
        let surface_x = style.page_padding.left as i32;
        let surface_width = 58 - style.page_padding.left - style.page_padding.right;
        let text_x = surface_x + style.code_block_padding as i32;

        assert!(block.lines.len() > 3, "the long source line should wrap");
        assert!(block.lines.iter().any(|line| line.fragments.is_empty()));
        assert!(block.lines.iter().any(|line| line.wrapped));
        assert!(block
            .lines
            .iter()
            .all(|line| line.bounds.top_left.x == surface_x
                && line.bounds.size.width == surface_width));
        assert_eq!(block.bounds.top_left.x, surface_x);
        assert_eq!(block.bounds.size.width, surface_width);
        assert!(block
            .lines
            .iter()
            .flat_map(|line| &line.fragments)
            .all(|fragment| fragment.bounds.top_left.x >= text_x));
    }

    #[test]
    fn long_link_is_split_into_multiple_targeted_fragments() {
        let target = NavigationTarget::DocumentAnchor {
            document: DocumentId::from("chapter.md"),
            anchor: "long-link".into(),
        };
        let document = Document::from_blocks(vec![Block::Paragraph(vec![Inline::Link {
            label: vec![Inline::Text("a-very-long-link-label-that-must-wrap".into())],
            destination: "chapter.md#long-link".into(),
            title: None,
        }])]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(42, 500));
        let fragments: Vec<_> = layout.blocks()[0]
            .lines
            .iter()
            .flat_map(|line| line.fragments.iter())
            .collect();
        assert!(fragments.len() > 2);
        assert!(fragments
            .iter()
            .all(|fragment| fragment.link.as_ref() == Some(&target)));
        assert!(fragments.iter().all(|fragment| fragment.style.underline));
        assert!(layout.blocks()[0]
            .lines
            .iter()
            .all(|line| line.bounds.size.width <= 38));
    }

    #[test]
    fn nested_lists_and_task_markers_are_indented_and_preserved() {
        let mut nested = ListItem::new(vec![Inline::Text("nested item".into())]);
        nested.task = TaskState::Unchecked;
        let mut parent = ListItem::new(vec![Inline::Text("parent item".into())]);
        parent.task = TaskState::Checked;
        parent.children.push(Block::List {
            ordered: false,
            start: 1,
            tight: true,
            items: vec![nested],
        });
        let document = Document::from_blocks(vec![Block::List {
            ordered: true,
            start: 1,
            tight: true,
            items: vec![parent],
        }]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(180, 300));
        let lines = &layout.blocks()[0].lines;
        assert!(all_text(lines).contains("parent item"));
        assert!(all_text(lines).contains("nested item"));
        assert!(!all_text(lines).contains("[x]"));
        assert!(!all_text(lines).contains("[ ]"));
        assert_eq!(
            lines
                .iter()
                .filter_map(|line| line.task)
                .collect::<Vec<_>>(),
            vec![TaskState::Checked, TaskState::Unchecked]
        );
        let checkbox_x = lines[0].task_checkbox_x.unwrap();
        assert!(lines[0]
            .fragments
            .iter()
            .any(|fragment| fragment.bounds.top_left.x > checkbox_x));
        assert!(lines[1].bounds.top_left.x > lines[0].bounds.top_left.x);
    }

    #[test]
    fn ordered_list_markers_preserve_source_start_and_increment_items() {
        let document = Document::from_blocks(vec![Block::List {
            ordered: true,
            start: 5,
            tight: true,
            items: vec![
                ListItem::new(vec![Inline::Text("first".into())]),
                ListItem::new(vec![Inline::Text("second".into())]),
            ],
        }]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(180, 300));

        assert_eq!(all_text(&layout.blocks()[0].lines), "5. first6. second");
    }

    #[test]
    fn nested_ordered_lists_keep_their_own_source_starts() {
        let mut outer_item = ListItem::new(vec![Inline::Text("outer".into())]);
        outer_item.children.push(Block::List {
            ordered: true,
            start: 8,
            tight: true,
            items: vec![
                ListItem::new(vec![Inline::Text("inner".into())]),
                ListItem::new(vec![Inline::Text("next inner".into())]),
            ],
        });
        let document = Document::from_blocks(vec![Block::List {
            ordered: true,
            start: 5,
            tight: true,
            items: vec![outer_item],
        }]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(180, 300));

        let text = all_text(&layout.blocks()[0].lines);
        assert!(text.contains("5. outer"));
        assert!(text.contains("8. inner"));
        assert!(text.contains("9. next inner"));
    }

    #[test]
    fn wrapped_unordered_items_use_a_stable_marker_column() {
        let document = Document::from_blocks(vec![Block::List {
            ordered: false,
            start: 1,
            tight: true,
            items: vec![ListItem::new(vec![Inline::Text(
                "wrapped unordered item keeps continuation text under the item text".into(),
            )])],
        }]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(78, 300));
        let lines = &layout.blocks()[0].lines;

        assert!(lines.len() >= 2);
        assert_eq!(lines[0].fragments[0].text, "• ");
        let text_x = lines[0].fragments[1].bounds.top_left.x;
        assert_eq!(lines[1].fragments[0].bounds.top_left.x, text_x);
        assert_eq!(
            lines[0].bounds.top_left.x,
            lines[0].fragments[0].bounds.top_left.x
        );
        assert_eq!(
            lines[0].fragments[0].bounds.top_left.x
                + lines[0].fragments[0].bounds.size.width as i32,
            text_x
        );
    }

    #[test]
    fn ordered_items_reserve_width_for_the_widest_marker() {
        let items = (1..=10)
            .map(|number| ListItem::new(vec![Inline::Text(format!("ordered item {number}"))]))
            .collect();
        let document = Document::from_blocks(vec![Block::List {
            ordered: true,
            start: 1,
            tight: true,
            items,
        }]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(220, 500));
        let lines = &layout.blocks()[0].lines;

        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0].fragments[0].text, "1. ");
        assert_eq!(lines[9].fragments[0].text, "10. ");
        assert_eq!(
            lines[0].fragments[1].bounds.top_left.x,
            lines[9].fragments[1].bounds.top_left.x
        );
    }

    #[test]
    fn task_markers_share_the_item_text_column() {
        let mut unchecked = ListItem::new(vec![Inline::Text(
            "unchecked task item with a continuation".into(),
        )]);
        unchecked.task = TaskState::Unchecked;
        let mut checked = ListItem::new(vec![Inline::Text("checked task item".into())]);
        checked.task = TaskState::Checked;
        let document = Document::from_blocks(vec![Block::List {
            ordered: false,
            start: 1,
            tight: true,
            items: vec![unchecked, checked],
        }]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(100, 300));
        let lines = &layout.blocks()[0].lines;
        let first_item_text_x = lines
            .iter()
            .find_map(|line| {
                line.fragments
                    .iter()
                    .find(|fragment| fragment.text.starts_with("unchecked"))
                    .map(|fragment| fragment.bounds.top_left.x)
            })
            .expect("unchecked task should be laid out");
        let second_item_line = lines
            .iter()
            .find(|line| {
                line.fragments
                    .iter()
                    .any(|fragment| fragment.text == "checked")
            })
            .expect("checked task should be laid out");

        assert_eq!(
            lines
                .iter()
                .filter_map(|line| line.task)
                .collect::<Vec<_>>(),
            vec![TaskState::Unchecked, TaskState::Checked]
        );
        assert!(lines.iter().any(|line| line.task_checkbox_x.is_some()));
        assert_eq!(
            second_item_line.fragments[0].bounds.top_left.x,
            first_item_text_x
        );
        assert!(lines
            .iter()
            .flat_map(|line| line.fragments.iter())
            .all(|fragment| !fragment.text.contains("[x]") && !fragment.text.contains("[ ]")));
    }

    #[test]
    fn list_continuation_keeps_text_column_after_a_page_break() {
        let style = ReaderStyle {
            page_padding: Insets::all(0),
            body: TextStyle::new(10, 10),
            heading: TextStyle {
                bold: true,
                ..TextStyle::new(10, 10)
            },
            paragraph_spacing: 0,
            heading_spacing_before: 0,
            heading_spacing_after: 0,
            list_item_spacing: 0,
            ..ReaderStyle::default()
        };
        let document = Document::from_blocks(vec![Block::List {
            ordered: false,
            start: 1,
            tight: true,
            items: vec![ListItem::new(vec![Inline::Text(
                "this list item wraps across a page boundary so its continuation starts on the next page".into(),
            )])],
        }]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(78, 10));
        assert!(layout.blocks()[0].lines.len() >= 2);

        let pages = Paginator::new(style).paginate(&layout);
        assert!(pages.len() >= 2);
        let first_text = pages[0]
            .display_list()
            .iter()
            .find_map(|command| match command {
                DisplayCommand::Text { text, bounds, .. } if text != "• " => Some(*bounds),
                _ => None,
            })
            .expect("first page should contain item text");
        let continuation_text = pages[1]
            .display_list()
            .iter()
            .find_map(|command| match command {
                DisplayCommand::Text { text, bounds, .. } if text != "• " => Some(*bounds),
                _ => None,
            })
            .expect("second page should contain continuation text");

        assert_eq!(continuation_text.top_left.x, first_text.top_left.x);
        assert!(!pages[1]
            .display_list()
            .iter()
            .any(|command| matches!(command, DisplayCommand::Text { text, .. } if text == "• ")));
    }

    #[test]
    fn tight_and_loose_lists_use_distinct_item_spacing() {
        let style = style();
        let tight = Block::List {
            ordered: false,
            start: 1,
            tight: true,
            items: vec![
                ListItem::new(vec![Inline::Text("tight one".into())]),
                ListItem::new(vec![Inline::Text("tight two".into())]),
            ],
        };
        let loose = Block::List {
            ordered: false,
            start: 1,
            tight: false,
            items: vec![
                ListItem::new(vec![Inline::Text("loose one".into())]),
                ListItem::new(vec![Inline::Text("loose two".into())]),
            ],
        };

        let tight_layout = LayoutEngine::new(style)
            .layout(&Document::from_blocks(vec![tight]), Viewport::new(180, 300));
        let loose_layout = LayoutEngine::new(style)
            .layout(&Document::from_blocks(vec![loose]), Viewport::new(180, 300));
        let tight_lines = &tight_layout.blocks()[0].lines;
        let loose_lines = &loose_layout.blocks()[0].lines;
        let tight_gap = tight_lines[1].bounds.top_left.y - rect_bottom(&tight_lines[0].bounds);
        let loose_gap = loose_lines[1].bounds.top_left.y - rect_bottom(&loose_lines[0].bounds);

        assert_eq!(tight_gap, style.tight_list_item_spacing as i32);
        assert_eq!(loose_gap, style.list_item_spacing as i32);
        assert!(loose_gap > tight_gap);
    }

    #[test]
    fn quote_retains_child_inline_styles_and_uses_inner_width() {
        let document = Document::from_blocks(vec![Block::Quote(vec![Block::Paragraph(vec![
            Inline::Strong(vec![Inline::Text("quoted text that wraps".into())]),
        ])])]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(100, 300));
        let block = &layout.blocks()[0];
        assert_eq!(block.kind, LayoutBlockKind::Quote);
        assert!(block.lines.iter().all(|line| line.bounds.top_left.x >= 12));
        assert!(block
            .lines
            .iter()
            .flat_map(|line| line.fragments.iter())
            .any(|fragment| fragment.style.bold));
    }

    #[test]
    fn headings_have_hierarchy_and_spacing_is_configurable() {
        let document = Document::from_blocks(vec![
            Block::heading(1, "Title"),
            Block::heading(3, "Section"),
            Block::paragraph("body"),
        ]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(300, 300));
        let h1 = &layout.blocks()[0];
        let h3 = &layout.blocks()[1];
        assert!(h1.lines[0].fragments[0].style.bold);
        assert!(
            h1.lines[0].fragments[0].style.font_size > h3.lines[0].fragments[0].style.font_size
        );
        assert_eq!(
            h3.bounds.top_left.y,
            h1.bounds.top_left.y
                + h1.bounds.size.height as i32
                + style().heading_spacing_after as i32
                + style().heading_spacing_before as i32
        );
    }

    #[test]
    fn wrapped_table_cells_keep_regular_paragraph_line_spacing() {
        let style = style();
        let text = "A table cell wraps this text across several lines while keeping the regular paragraph rhythm.";
        let document = Document::from_blocks(vec![
            Block::paragraph(text),
            Block::Table(Table {
                headers: Vec::new(),
                rows: vec![vec![vec![Inline::Text(text.into())]]],
                alignments: Vec::new(),
            }),
        ]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(60, 300));
        let paragraph = &layout.blocks()[0].lines;
        let table_block = &layout.blocks()[1];
        let table = table_block.table.as_ref().expect("table metadata");
        let row = &table.rows[0];
        let table_lines = &table_block.lines[row.line_range.clone()];
        let paragraph_y = paragraph
            .iter()
            .filter_map(|line| {
                line.fragments
                    .first()
                    .map(|fragment| fragment.bounds.top_left.y)
            })
            .collect::<Vec<_>>();
        let table_y = table_lines
            .iter()
            .filter_map(|line| {
                line.fragments
                    .first()
                    .map(|fragment| fragment.bounds.top_left.y)
            })
            .collect::<Vec<_>>();

        assert!(paragraph_y.len() >= 2);
        assert!(table_y.len() >= 2);
        assert!(paragraph_y
            .windows(2)
            .all(|pair| pair[1] - pair[0] == style.body.line_height as i32));
        assert!(table_y
            .windows(2)
            .all(|pair| { pair[1] - pair[0] == style.body.line_height as i32 }));
        assert_eq!(
            table_y[0] - table_lines[0].bounds.top_left.y,
            style.table_cell_vertical_padding as i32
        );
        assert_eq!(
            rect_bottom(&table_lines[table_lines.len() - 1].bounds)
                - table_y[table_y.len() - 1]
                - style.body.line_height as i32,
            style.table_cell_vertical_padding as i32
        );
    }

    #[test]
    fn tables_allocate_columns_and_keep_cell_content_separate() {
        let table = Table {
            headers: vec![
                vec![Inline::Text("Name".into())],
                vec![Inline::Text("Status".into())],
                vec![Inline::Text("Owner".into())],
            ],
            rows: vec![vec![
                vec![Inline::Text("Parser".into())],
                vec![Inline::Text("ready".into())],
                vec![Inline::Link {
                    label: vec![Inline::Text("design-doc".into())],
                    destination: "docs/design.md".into(),
                    title: None,
                }],
            ]],
            alignments: vec![
                crate::document::TableAlignment::Left,
                crate::document::TableAlignment::Center,
                crate::document::TableAlignment::Right,
            ],
        };
        let document = Document::from_blocks(vec![Block::Table(table)]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(180, 200));
        let block = &layout.blocks()[0];
        let table_layout = block.table.as_ref().expect("table metadata");

        assert_eq!(table_layout.mode, TableLayoutMode::Normal);
        assert_eq!(table_layout.groups.len(), 1);
        assert_eq!(table_layout.rows.len(), 2);
        assert!(table_layout
            .columns
            .iter()
            .all(|column| column.preferred_width >= column.minimum_width));
        assert!(block
            .lines
            .iter()
            .flat_map(|line| line.fragments.iter())
            .any(|fragment| fragment.text == "Parser"));
        assert!(block
            .lines
            .iter()
            .flat_map(|line| line.fragments.iter())
            .any(|fragment| fragment.link.is_some()));
        assert!(
            block.lines[0].fragments[1].bounds.top_left.x
                > block.lines[0].fragments[0].bounds.top_left.x
        );
    }

    #[test]
    fn tables_use_content_aware_grouping_at_t1_width() {
        let document = crate::parse::parse(include_str!("../../tests/fixtures/table-layout.md"))
            .expect("table fixture should parse");
        let style = ReaderStyle::default();
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(600, 800));
        let tables = layout
            .blocks()
            .iter()
            .filter_map(|block| block.table.as_ref())
            .collect::<Vec<_>>();

        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].mode, TableLayoutMode::Normal);
        assert_eq!(tables[1].mode, TableLayoutMode::Grouped);
        let detail_group = tables[1]
            .groups
            .iter()
            .find(|group| group.columns.contains(&7))
            .expect("wide table should retain a detail continuation group");
        let detail_width = detail_group
            .columns
            .iter()
            .position(|column| *column == 7)
            .map(|index| detail_group.widths[index])
            .expect("detail group should include its detail width");
        assert!(detail_group.columns.contains(&0));
        assert!(detail_width > tables[1].columns[7].minimum_width);
        assert!(detail_width > tables[1].groups[0].widths.iter().copied().max().unwrap());

        for block in layout
            .blocks()
            .iter()
            .filter(|block| block.kind == LayoutBlockKind::Table)
        {
            let table = block.table.as_ref().expect("table metadata");
            for row in &table.rows {
                for (line_index, line) in block.lines[row.line_range.clone()].iter().enumerate() {
                    for fragment in &line.fragments {
                        assert!(fragment.style.font_size >= style.table_min_font_size);
                        if line_index == 0 {
                            assert_eq!(
                                fragment.bounds.top_left.y - line.bounds.top_left.y,
                                style.table_cell_vertical_padding as i32
                            );
                        } else {
                            assert_eq!(fragment.bounds.top_left.y, line.bounds.top_left.y);
                        }
                        if line_index + 1 == row.line_range.len() {
                            assert!(rect_bottom(&fragment.bounds) < rect_bottom(&line.bounds));
                        }
                    }
                }
            }
        }

        let first_table = layout
            .blocks()
            .iter()
            .find(|block| block.kind == LayoutBlockKind::Table)
            .expect("first table block");
        let first_header = first_table.table.as_ref().unwrap().rows[0].line_range.start;
        assert_eq!(
            first_table.lines[first_header].bounds.size.height,
            style
                .body
                .line_height
                .saturating_add(style.table_cell_vertical_padding.saturating_mul(2))
        );
    }

    #[test]
    fn fitting_five_column_table_keeps_normal_typography() {
        let style = ReaderStyle::default();
        let document = Document::from_blocks(vec![Block::Table(Table {
            headers: (1..=5)
                .map(|index| vec![Inline::Text(format!("H{index}"))])
                .collect(),
            rows: vec![vec![
                vec![Inline::Text("one".into())],
                vec![Inline::Text("two".into())],
                vec![Inline::Text("three".into())],
                vec![Inline::Text("four".into())],
                vec![Inline::Text("five".into())],
            ]],
            alignments: Vec::new(),
        })]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(600, 300));
        let table = layout.blocks()[0].table.as_ref().expect("table metadata");

        assert_eq!(table.mode, TableLayoutMode::Normal);
        assert!(layout.blocks()[0]
            .lines
            .iter()
            .flat_map(|line| line.fragments.iter())
            .all(|fragment| fragment.style.font_size == style.body.font_size));
        assert!(layout.blocks()[0].lines.iter().all(|line| {
            line.bounds.size.height
                == style
                    .body
                    .line_height
                    .saturating_add(style.table_cell_vertical_padding.saturating_mul(2))
        }));
    }

    #[test]
    fn tables_use_key_column_groups_when_the_frame_cannot_fit() {
        let document = Document::from_blocks(vec![Block::Table(Table {
            headers: (0..8)
                .map(|index| vec![Inline::Text(format!("H{index}"))])
                .collect(),
            rows: vec![vec![
                vec![Inline::Text("key".into())],
                vec![Inline::Text("one".into())],
                vec![Inline::Text("two".into())],
                vec![Inline::Text("three".into())],
                vec![Inline::Text("four".into())],
                vec![Inline::Text("five".into())],
                vec![Inline::Text("six".into())],
                vec![Inline::Text("seven".into())],
            ]],
            alignments: Vec::new(),
        })]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(60, 300));
        let table_layout = layout.blocks()[0].table.as_ref().unwrap();

        assert_eq!(table_layout.mode, TableLayoutMode::Grouped);
        assert!(table_layout.groups.len() > 1);
        assert!(table_layout
            .groups
            .iter()
            .skip(1)
            .all(|group| group.columns.contains(&0)));
    }

    #[test]
    fn zero_and_narrow_viewports_are_deterministic() {
        let document = Document::from_blocks(vec![Block::paragraph("narrow")]);
        for viewport in [
            Viewport::new(0, 0),
            Viewport::new(1, 1),
            Viewport::new(2, 1),
        ] {
            let layout = LayoutEngine::new(ReaderStyle::default()).layout(&document, viewport);
            assert_eq!(layout.blocks().len(), 1);
            assert!(!layout.blocks()[0].lines.is_empty());
        }
    }

    #[test]
    fn parsed_reader_fixture_is_deterministic_at_arbitrary_widths() {
        let document = crate::parse::parse(include_str!("../../tests/fixtures/agent-output.md"))
            .expect("fixture should parse");
        let style = ReaderStyle {
            page_padding: Insets::all(4),
            list_indent: 10,
            block_quote_indent: 8,
            block_quote_padding: 2,
            ..ReaderStyle::default()
        };

        for width in [24, 47, 96, 240] {
            let layout = LayoutEngine::new(style).layout(&document, Viewport::new(width, 120));
            assert!(!layout.blocks().is_empty());
            assert!(layout
                .blocks()
                .iter()
                .flat_map(|block| block.lines.iter())
                .all(|line| line.bounds.size.width <= width.max(1)));
        }
    }
}
