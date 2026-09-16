//! Width-constrained, viewport-relative document layout.
//!
//! Layout deliberately does not know about pages. A [`DocumentLayout`]
//! contains every positioned line in document coordinates; [`crate::pagination`]
//! can consequently split it at legal line boundaries later. Every advance
//! used by the wrapper comes from [`TextMeasurer`]. In production that is
//! normally [`crate::typography::FontdueTextEngine`], while the deterministic
//! approximate measurer keeps the structural tests independent of font files.

use crate::document::{Block, Inline, ListItem, Table, TaskState};
pub use crate::geometry::Viewport;
use crate::highlighting::{CodeHighlighter, SyntectHighlighter};
use crate::navigation::NavigationTarget;
use crate::style::{ReaderStyle, TextStyle};
use embedded_graphics::geometry::{Point, Size};
use embedded_graphics::primitives::Rectangle;

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
    /// A linked span is repeated on every line containing its visible text.
    /// Pagination turns each fragment into a separate hit region.
    pub link: Option<NavigationTarget>,
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
        let mut blocks = Vec::new();
        let mut cursor_y = self.style.page_padding.top as i32;
        let content_width = viewport.width.saturating_sub(
            self.style
                .page_padding
                .left
                .saturating_add(self.style.page_padding.right),
        );
        let content_x = self.style.page_padding.left as i32;

        for block in document.blocks() {
            cursor_y = cursor_y.saturating_add(self.spacing_before(block));
            let (layout_block, block_bottom) =
                self.layout_block(block, cursor_y, content_x, content_width);
            cursor_y = block_bottom.saturating_add(self.spacing_after(block));
            blocks.push(layout_block);
        }

        DocumentLayout { viewport, blocks }
    }

    fn layout_block(&self, block: &Block, start_y: i32, x: i32, width: u32) -> (LayoutBlock, i32) {
        let (kind, anchor, lines) = self.layout_content(block, start_y, x, width);
        let bounds = block_bounds(&lines, x, start_y);
        let bottom = rect_bottom(&bounds);
        (
            LayoutBlock {
                kind,
                bounds,
                lines,
                anchor,
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
    ) -> (LayoutBlockKind, Option<String>, Vec<LayoutLine>) {
        match block {
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
            // Tables are intentionally represented as readable pipe-separated
            // rows until table-specific layout defines columns.
            Block::Table(table) => (
                LayoutBlockKind::Table,
                None,
                self.table_lines(table, start_y, x, width),
            ),
            // Syntax highlighting is a separate concern. Preserve code text,
            // line breaks, indentation, and a monospace style here.
            Block::CodeBlock { language, code, .. } => (
                LayoutBlockKind::Code,
                None,
                self.code_lines(language.as_deref(), code, start_y, x, width),
            ),
            // Image decoding is separate; an alt-text placeholder is still
            // useful and deterministic for the first reader.
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
                    wrapped: false,
                }],
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
        wrap_spans(
            &self.measurer,
            spans,
            start_y,
            x,
            width,
            base_style.line_height,
        )
    }

    fn list_lines(
        &self,
        ordered: bool,
        items: &[ListItem],
        start_y: i32,
        x: i32,
        width: u32,
    ) -> Vec<LayoutLine> {
        let mut result = Vec::new();
        self.append_list_lines(ordered, items, start_y, x, width, &mut result);
        if result.is_empty() {
            result.push(empty_line(x, start_y, self.style.body.line_height));
        }
        result
    }

    fn append_list_lines(
        &self,
        ordered: bool,
        items: &[ListItem],
        start_y: i32,
        x: i32,
        width: u32,
        output: &mut Vec<LayoutLine>,
    ) -> i32 {
        let indent = self.style.list_indent.min(width);
        let item_x = x.saturating_add(indent as i32);
        let item_width = width.saturating_sub(indent);
        let mut y = start_y;

        for (index, item) in items.iter().enumerate() {
            let marker = list_marker(ordered, index, item.task);
            let mut spans = vec![Span::new(marker, self.style.body, None, false)];
            for inline in &item.content {
                collect_spans(inline, self.style.body, None, &mut spans);
            }
            let lines = wrap_spans(
                &self.measurer,
                spans,
                y,
                item_x,
                item_width,
                self.style.body.line_height,
            );
            y = append_lines(output, lines, y);

            // Children retain their block semantics and can themselves contain
            // paragraphs, quotes, or another list. Their x origin advances once
            // per nesting level, and each child line remains a legal split.
            for child in &item.children {
                y = y.saturating_add(self.spacing_before(child));
                let (_, _, lines) = self.layout_content(child, y, item_x, item_width);
                y = append_lines(output, lines, y);
                y = y.saturating_add(self.spacing_after(child));
            }
            y = y.saturating_add(self.style.list_item_spacing as i32);
        }

        y
    }

    fn quote_lines(&self, blocks: &[Block], start_y: i32, x: i32, width: u32) -> Vec<LayoutLine> {
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
            let (_, _, lines) = self.layout_content(block, y, inner_x, inner_width);
            y = append_lines(&mut result, lines, y);
            y = y.saturating_add(self.spacing_after(block));
        }

        if result.is_empty() {
            result.push(empty_line(inner_x, start_y, self.style.body.line_height));
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
            y = append_lines(&mut result, lines, y);
        }
        if result.is_empty() {
            result.push(empty_line(x, start_y, self.style.body.line_height));
        }
        result
    }

    fn code_lines(
        &self,
        language: Option<&str>,
        code: &str,
        start_y: i32,
        x: i32,
        width: u32,
    ) -> Vec<LayoutLine> {
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
                x,
                width,
                self.style.code.line_height,
            );
            for (line_index, line) in lines.iter_mut().enumerate() {
                line.wrapped = line_index > 0;
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
            result.push(empty_line(x, start_y, self.style.code.line_height));
        }
        result
    }
}

#[derive(Clone, Debug)]
struct Span {
    text: String,
    style: TextStyle,
    link: Option<NavigationTarget>,
    preserve_whitespace: bool,
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
        }
    }
}

fn collect_spans(
    inline: &Inline,
    style: TextStyle,
    inherited_link: Option<NavigationTarget>,
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
                collect_spans(
                    child,
                    TextStyle {
                        strikethrough: true,
                        ..style
                    },
                    inherited_link.clone(),
                    spans,
                );
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
        Inline::Image { alt, .. } => spans.push(Span::new(
            format!("[image: {alt}]"),
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

fn list_marker(ordered: bool, index: usize, task: TaskState) -> String {
    let ordinary = if ordered {
        format!("{}. ", index + 1)
    } else {
        "• ".to_owned()
    };
    match task {
        TaskState::None => ordinary,
        TaskState::Unchecked => {
            if ordered {
                format!("{}. [ ] ", index + 1)
            } else {
                "[ ] ".to_owned()
            }
        }
        TaskState::Checked => {
            if ordered {
                format!("{}. [x] ", index + 1)
            } else {
                "[x] ".to_owned()
            }
        }
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
        assert!(fragments.iter().any(|fragment| fragment.text == " "));
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
            items: vec![nested],
        });
        let document = Document::from_blocks(vec![Block::List {
            ordered: true,
            items: vec![parent],
        }]);
        let layout = LayoutEngine::new(style()).layout(&document, Viewport::new(180, 300));
        let lines = &layout.blocks()[0].lines;
        assert!(all_text(lines).contains("1. [x] parent item"));
        assert!(all_text(lines).contains("[ ] nested item"));
        assert!(lines[1].bounds.top_left.x > lines[0].bounds.top_left.x);
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
