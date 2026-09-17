//! Pagination and semantic display-list construction.
//!
//! A [`PageLayout`] is the handoff from layout to rendering. Its commands
//! are ordered back-to-front, use page-space coordinates, and contain no
//! Markdown or device concepts. The same page can consequently be rendered
//! by a framebuffer adapter, a host test target, or another renderer.

use crate::geometry::{translate, Rect, Viewport};
use crate::image::RasterImage;
use crate::layout::{DocumentLayout, LayoutBlockKind, LayoutLine, TableLayout, TableRowLayout};
use crate::navigation::NavigationTarget;
use crate::style::{BorderStyle, Color, FillStyle, ReaderStyle, TextStyle};
use embedded_graphics::geometry::Point;
use std::ops::{Deref, DerefMut};

/// A stable position between the laid-out lines of a document.
///
/// `block` identifies a top-level document block and `line` identifies a
/// line within that block.  A cursor at `(block, 0)` is before the first line
/// of that block.  The canonical end cursor is `(block_count, 0)`; using the
/// next block for a boundary avoids having two different cursors for the same
/// position between blocks.  The cursor is independent of page pixels and can
/// therefore be used to restore a reading position after a page is rebuilt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentCursor {
    pub block: usize,
    pub line: usize,
}

impl DocumentCursor {
    pub const fn new(block: usize, line: usize) -> Self {
        Self { block, line }
    }
}

/// A half-open logical range of laid-out content: `[start, end)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DocumentRange {
    pub start: DocumentCursor,
    pub end: DocumentCursor,
}

impl DocumentRange {
    pub const fn new(start: DocumentCursor, end: DocumentCursor) -> Self {
        Self { start, end }
    }

    pub const fn empty(cursor: DocumentCursor) -> Self {
        Self {
            start: cursor,
            end: cursor,
        }
    }

    pub fn contains(&self, cursor: DocumentCursor) -> bool {
        self.start <= cursor && cursor < self.end
    }

    pub const fn start(self) -> DocumentCursor {
        self.start
    }

    pub const fn end(self) -> DocumentCursor {
        self.end
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Short aliases for callers that describe pagination in logical-position
/// terminology.
pub type LogicalPosition = DocumentCursor;
pub type LogicalRange = DocumentRange;

/// An ordered display list. Commands later in the list are drawn on top of
/// earlier commands.
pub type DisplayList = Vec<DisplayCommand>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageLayout {
    pub number: usize,
    pub viewport: Viewport,
    /// The logical content represented by this page, independent of pixels.
    pub range: DocumentRange,
    /// Commands are in back-to-front order and are expressed in page space.
    pub commands: DisplayList,
    /// Each entry is one page-space rectangle. Multiple entries may have the
    /// same target when a linked span wraps across lines.
    pub hit_regions: Vec<HitRegion>,
}

impl PageLayout {
    pub fn new(number: usize, viewport: Viewport) -> Self {
        Self {
            number,
            viewport,
            range: DocumentRange::default(),
            commands: DisplayList::new(),
            hit_regions: Vec::new(),
        }
    }

    pub fn with_range(number: usize, viewport: Viewport, range: DocumentRange) -> Self {
        Self {
            number,
            viewport,
            range,
            commands: DisplayList::new(),
            hit_regions: Vec::new(),
        }
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn display_list(&self) -> &DisplayList {
        &self.commands
    }

    pub fn logical_range(&self) -> DocumentRange {
        self.range
    }

    pub fn range(&self) -> DocumentRange {
        self.range
    }

    pub fn start_cursor(&self) -> DocumentCursor {
        self.range.start
    }

    pub fn end_cursor(&self) -> DocumentCursor {
        self.range.end
    }

    /// Add a command after clipping its bounds to the page viewport.
    ///
    /// Clipping here gives renderers a useful invariant while still allowing
    /// them to clip again when the page is translated to a larger target.
    pub fn push_command(&mut self, command: DisplayCommand) {
        let Some(bounds) = self.viewport.clip(command.bounds()) else {
            return;
        };
        self.commands.push(command.with_bounds(bounds));
    }

    /// Add one semantic hit rectangle, clipped to page space.
    pub fn add_hit_region(&mut self, region: HitRegion) {
        let Some(bounds) = self.viewport.clip(region.bounds) else {
            return;
        };
        self.hit_regions.push(HitRegion { bounds, ..region });
    }

    /// Find the topmost semantic region containing `point`.
    ///
    /// Hit regions follow display-list ordering: later regions are topmost,
    /// so reverse iteration makes overlap behavior deterministic.
    pub fn hit_test(&self, point: Point) -> Option<&HitRegion> {
        if !self.viewport.contains(point) {
            return None;
        }
        self.hit_regions
            .iter()
            .rev()
            .find(|region| region.bounds.contains(point))
    }
}

/// The complete logical pagination result.
///
/// It dereferences to a page slice for compatibility with callers that only
/// need indexing or iteration. The explicit methods are useful to navigation
/// code that should not know how pages are stored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    pages: Vec<PageLayout>,
}

impl Pagination {
    fn new(pages: Vec<PageLayout>) -> Self {
        Self { pages }
    }

    pub fn pages(&self) -> &[PageLayout] {
        &self.pages
    }

    pub fn page(&self, index: usize) -> Option<&PageLayout> {
        self.pages.get(index)
    }

    pub fn page_mut(&mut self, index: usize) -> Option<&mut PageLayout> {
        self.pages.get_mut(index)
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn page_count(&self) -> usize {
        self.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Return the page containing a line-start cursor.
    pub fn page_for_cursor(&self, cursor: DocumentCursor) -> Option<&PageLayout> {
        self.pages.iter().find(|page| page.range.contains(cursor))
    }

    pub fn page_index_for_cursor(&self, cursor: DocumentCursor) -> Option<usize> {
        self.pages
            .iter()
            .position(|page| page.range.contains(cursor))
    }

    pub fn next_page_index(&self, index: usize) -> Option<usize> {
        index.checked_add(1).filter(|next| *next < self.pages.len())
    }

    pub fn previous_page_index(&self, index: usize) -> Option<usize> {
        (index > 0 && index < self.pages.len()).then_some(index - 1)
    }

    pub fn next_page(&self, index: usize) -> Option<&PageLayout> {
        self.next_page_index(index).and_then(|next| self.page(next))
    }

    pub fn previous_page(&self, index: usize) -> Option<&PageLayout> {
        self.previous_page_index(index)
            .and_then(|previous| self.page(previous))
    }

    /// Retained for source compatibility with the original `Vec` return type.
    pub fn remove(&mut self, index: usize) -> PageLayout {
        self.pages.remove(index)
    }

    pub fn into_pages(self) -> Vec<PageLayout> {
        self.pages
    }
}

impl Deref for Pagination {
    type Target = [PageLayout];

    fn deref(&self) -> &Self::Target {
        &self.pages
    }
}

impl DerefMut for Pagination {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.pages
    }
}

/// A compact page directory used by bounded readers.
///
/// Unlike [`Pagination`], this directory retains only the logical range and
/// document-space origin needed to rebuild one display list. The reader can
/// consequently evict old `PageLayout` values without losing page navigation
/// or forcing a Markdown reparse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaginationIndex {
    viewport: Viewport,
    pages: Vec<PageIndexEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PageIndexEntry {
    range: DocumentRange,
    origin_y: i32,
    repeated_header: Option<(usize, std::ops::Range<usize>)>,
}

impl PaginationIndex {
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn len(&self) -> usize {
        self.page_count()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn range(&self, index: usize) -> Option<DocumentRange> {
        self.pages.get(index).map(|page| page.range)
    }

    pub fn page_index_for_cursor(&self, cursor: DocumentCursor) -> Option<usize> {
        self.pages
            .iter()
            .position(|page| page.range.contains(cursor))
    }

    fn from_pagination(
        layout: &DocumentLayout,
        pagination: &Pagination,
        style: &ReaderStyle,
    ) -> Self {
        let pages = pagination
            .pages()
            .iter()
            .map(|page| {
                let (origin_y, repeated_header) = page_origin_and_header(layout, page, style);
                PageIndexEntry {
                    range: page.range,
                    origin_y,
                    repeated_header,
                }
            })
            .collect();
        Self {
            viewport: layout.viewport,
            pages,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayCommand {
    /// A positioned text run. The renderer chooses how to rasterize it.
    Text {
        bounds: Rect,
        text: String,
        style: TextStyle,
    },
    /// A solid background or other rectangular fill.
    Fill { bounds: Rect, style: FillStyle },
    /// A rectangular border; the interior is left unchanged.
    Border { bounds: Rect, style: BorderStyle },
    /// A horizontal or vertical rule represented as a stroked rectangle.
    Rule { bounds: Rect, style: BorderStyle },
    /// A deterministic fallback for unavailable image data.
    ImagePlaceholder {
        bounds: Rect,
        source: Option<String>,
        alt: String,
    },
    /// A bounded grayscale image ready for the generic renderer.
    Image {
        bounds: Rect,
        image: RasterImage,
        source: String,
        alt: String,
    },
}

impl DisplayCommand {
    pub fn bounds(&self) -> Rect {
        match self {
            Self::Text { bounds, .. }
            | Self::Fill { bounds, .. }
            | Self::Border { bounds, .. }
            | Self::Rule { bounds, .. }
            | Self::ImagePlaceholder { bounds, .. }
            | Self::Image { bounds, .. } => *bounds,
        }
    }

    fn with_bounds(self, bounds: Rect) -> Self {
        match self {
            Self::Text { text, style, .. } => Self::Text {
                bounds,
                text,
                style,
            },
            Self::Fill { style, .. } => Self::Fill { bounds, style },
            Self::Border { style, .. } => Self::Border { bounds, style },
            Self::Rule { style, .. } => Self::Rule { bounds, style },
            Self::ImagePlaceholder { source, alt, .. } => Self::ImagePlaceholder {
                bounds,
                source,
                alt,
            },
            Self::Image {
                image, source, alt, ..
            } => Self::Image {
                bounds,
                image,
                source,
                alt,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitRegion {
    pub bounds: Rect,
    pub target: NavigationTarget,
}

impl HitRegion {
    pub fn new(bounds: Rect, target: NavigationTarget) -> Self {
        Self { bounds, target }
    }
}

pub struct Paginator {
    pub style: ReaderStyle,
}

#[derive(Clone, Copy)]
struct PageContext {
    start: Option<DocumentCursor>,
    origin_y: i32,
    height: i32,
}

impl Default for Paginator {
    fn default() -> Self {
        Self::new(ReaderStyle::default())
    }
}

impl Paginator {
    pub fn new(style: ReaderStyle) -> Self {
        Self { style }
    }

    /// Paginate a complete layout into logical page ranges and display lists.
    ///
    /// Pages are formed by a deterministic greedy scan of positioned lines.
    /// A line whose bottom exactly reaches the usable page bottom belongs to
    /// that page. Page-space coordinates are reset at every boundary, while
    /// each page retains the source cursor range that produced it.
    pub fn paginate(&self, layout: &DocumentLayout) -> Pagination {
        let mut pages = Vec::new();
        let mut page_origin_y = 0;
        let mut page = PageLayout::new(1, layout.viewport);
        let mut page_start = None;
        let page_height = layout.viewport.height.max(1) as i32;
        let document_end = DocumentCursor::new(layout.blocks().len(), 0);

        for (block_index, block) in layout.blocks().iter().enumerate() {
            for (line_index, line) in block.lines.iter().enumerate() {
                let cursor = DocumentCursor::new(block_index, line_index);
                if self.should_break_before(
                    layout,
                    block_index,
                    line_index,
                    line,
                    PageContext {
                        start: page_start,
                        origin_y: page_origin_y,
                        height: page_height,
                    },
                ) && page_start.is_some()
                {
                    page.range = DocumentRange::new(page_start.unwrap(), cursor);
                    pages.push(page);
                    page_origin_y = line
                        .bounds
                        .top_left
                        .y
                        .saturating_sub(self.style.page_padding.top as i32);
                    page = PageLayout::new(pages.len() + 1, layout.viewport);
                    if block.kind == LayoutBlockKind::Table {
                        if let Some(table) = block.table.as_ref().filter(|table| {
                            table_row_at(table, line_index).is_some_and(|row| !row.header)
                        }) {
                            if let Some(header) = table_header_for_line(table, line_index) {
                                let header_height = table_row_height(block, header);
                                page_origin_y = line
                                    .bounds
                                    .top_left
                                    .y
                                    .saturating_sub(self.style.page_padding.top as i32)
                                    .saturating_sub(header_height);
                                let header_offset = Point::new(
                                    0,
                                    self.style.page_padding.top as i32
                                        - block.lines[header.line_range.start].bounds.top_left.y,
                                );
                                for header_index in header.line_range.clone() {
                                    add_line(
                                        &mut page,
                                        &block.lines[header_index],
                                        header_offset,
                                        block.kind,
                                        &self.style,
                                        block.table.as_ref(),
                                        header_index,
                                    );
                                }
                            }
                        }
                    }
                    page_start = None;
                }
                page_start.get_or_insert(cursor);
                add_line(
                    &mut page,
                    line,
                    Point::new(0, page_origin_y.saturating_neg()),
                    block.kind,
                    &self.style,
                    block.table.as_ref(),
                    line_index,
                );
            }
        }

        if let Some(start) = page_start {
            page.range = DocumentRange::new(start, document_end);
            pages.push(page);
        } else if pages.is_empty() {
            pages.push(PageLayout::with_range(
                1,
                layout.viewport,
                DocumentRange::empty(document_end),
            ));
        }
        Pagination::new(pages)
    }

    /// Build a compact page directory. Pagination is performed once to retain
    /// the established split rules, then the temporary display lists are
    /// released. Bounded readers rebuild only requested pages from this
    /// directory and the retained document layout.
    pub fn index(&self, layout: &DocumentLayout) -> PaginationIndex {
        let pagination = self.paginate(layout);
        PaginationIndex::from_pagination(layout, &pagination, &self.style)
    }

    /// Rebuild one page from a compact [`PaginationIndex`] entry.
    pub fn page_from_index(
        &self,
        layout: &DocumentLayout,
        index: &PaginationIndex,
        page_index: usize,
    ) -> Option<PageLayout> {
        let entry = index.pages.get(page_index)?;
        let mut page = PageLayout::with_range(page_index + 1, index.viewport, entry.range);

        if let Some((block_index, header_range)) = &entry.repeated_header {
            let block = layout.blocks().get(*block_index)?;
            let header = block.lines.get(header_range.start)?;
            let header_offset = Point::new(
                0,
                self.style.page_padding.top as i32 - header.bounds.top_left.y,
            );
            for line_index in header_range.clone() {
                let line = block.lines.get(line_index)?;
                add_line(
                    &mut page,
                    line,
                    header_offset,
                    block.kind,
                    &self.style,
                    block.table.as_ref(),
                    line_index,
                );
            }
        }

        for (block_index, block) in layout.blocks().iter().enumerate() {
            for (line_index, line) in block.lines.iter().enumerate() {
                let cursor = DocumentCursor::new(block_index, line_index);
                if entry.range.contains(cursor) {
                    add_line(
                        &mut page,
                        line,
                        Point::new(0, entry.origin_y.saturating_neg()),
                        block.kind,
                        &self.style,
                        block.table.as_ref(),
                        line_index,
                    );
                }
            }
        }
        Some(page)
    }

    fn should_break_before(
        &self,
        layout: &DocumentLayout,
        block_index: usize,
        line_index: usize,
        line: &LayoutLine,
        context: PageContext,
    ) -> bool {
        let Some(page_start) = context.start else {
            return false;
        };
        let page_limit = context
            .origin_y
            .saturating_add(context.height)
            .saturating_sub(self.style.page_padding.bottom as i32);
        let line_fits = line_bottom(line.bounds) <= page_limit;
        let block = &layout.blocks()[block_index];

        if block.kind == LayoutBlockKind::Table {
            if let Some(table) = block.table.as_ref() {
                if let Some(row) = table_row_at(table, line_index) {
                    if row.line_range.start == line_index {
                        let row_bottom = block
                            .lines
                            .get(row.line_range.end.saturating_sub(1))
                            .map(|line| line_bottom(line.bounds))
                            .unwrap_or(line_bottom(line.bounds));
                        // A normal row moves as a unit. If a single row is
                        // taller than a page, allow its lines to split after
                        // the row has been placed once; this is the explicit
                        // oversized-row fallback and guarantees progress.
                        if row_bottom > page_limit
                            && page_start != DocumentCursor::new(block_index, line_index)
                        {
                            return true;
                        }
                    }
                }
            }
        }

        if !line_fits {
            // If an image is taller than a page, keep its already-started
            // placeholder together as the documented oversized-atomic
            // fallback. A normal image starts at line zero and will have been
            // moved before this branch when the whole block fits a page.
            if block.kind == LayoutBlockKind::Image
                && line_index > 0
                && page_start.block == block_index
                && page_start.line == 0
            {
                return false;
            }
            return true;
        }

        // Rules and images are atomic when they can fit on a fresh page. An
        // over-height atomic block is emitted on one page and clipped by the
        // ordinary display-list boundary rather than being silently dropped.
        if matches!(block.kind, LayoutBlockKind::Rule | LayoutBlockKind::Image)
            && line_index == 0
            && block_bottom(block) > page_limit
        {
            return true;
        }

        // A heading at the bottom is only useful when at least the next line
        // can follow it. If it cannot, move the heading as a unit when the
        // heading itself fits a fresh page. A very tall heading falls back to
        // the ordinary line-by-line rule.
        if block.kind == LayoutBlockKind::Heading && line_index == 0 {
            if let Some(next) = next_line(layout, block_index, line_index) {
                if line_bottom(next.bounds) > page_limit
                    && block_fits_fresh_page(block, context.height, &self.style)
                {
                    return true;
                }
            }
        }

        if block.kind == LayoutBlockKind::Paragraph {
            let started_on_page = page_start.block < block_index
                || (page_start.block == block_index && page_start.line == 0);
            if started_on_page {
                // Keep a short paragraph together when the fresh-page fit is
                // available. This also prevents the common one-line orphan
                // after a heading or preceding paragraph.
                if line_index == 0
                    && block_fits_fresh_page(block, context.height, &self.style)
                    && block_bottom(block) > page_limit
                {
                    return true;
                }

                // Look one line ahead before filling the last available slot.
                // If the final two lines cannot fit together, move both to the
                // next page rather than producing a one-line paragraph page.
                if line_index + 2 == block.lines.len() {
                    if let Some(last) = block.lines.get(line_index + 1) {
                        if line_bottom(last.bounds) > page_limit {
                            return true;
                        }
                    }
                }
            }
        }

        false
    }
}

fn next_line(
    layout: &DocumentLayout,
    block_index: usize,
    line_index: usize,
) -> Option<&LayoutLine> {
    let block = &layout.blocks()[block_index];
    block
        .lines
        .get(line_index + 1)
        .or_else(|| layout.blocks().get(block_index + 1)?.lines.first())
}

fn block_bottom(block: &crate::layout::LayoutBlock) -> i32 {
    block
        .lines
        .last()
        .map(|line| line_bottom(line.bounds))
        .unwrap_or(block.bounds.top_left.y)
}

fn block_fits_fresh_page(
    block: &crate::layout::LayoutBlock,
    page_height: i32,
    style: &ReaderStyle,
) -> bool {
    let Some(first) = block.lines.first() else {
        return true;
    };
    let block_height = block_bottom(block).saturating_sub(first.bounds.top_left.y);
    let available = page_height
        .saturating_sub(style.page_padding.top as i32)
        .saturating_sub(style.page_padding.bottom as i32)
        .max(1);
    block_height <= available
}

fn add_line(
    page: &mut PageLayout,
    line: &LayoutLine,
    page_offset: Point,
    kind: LayoutBlockKind,
    style: &ReaderStyle,
    table: Option<&TableLayout>,
    line_index: usize,
) {
    let code_surface = kind == LayoutBlockKind::Code || line.code;
    if matches!(kind, LayoutBlockKind::Quote | LayoutBlockKind::Alert) {
        let border_x = line.bounds.top_left.x.saturating_sub(
            style
                .block_quote_indent
                .saturating_add(style.block_quote_padding) as i32,
        );
        page.push_command(DisplayCommand::Rule {
            bounds: translate(
                Rect::new(
                    Point::new(border_x, line.bounds.top_left.y),
                    embedded_graphics::geometry::Size::new(
                        style.block_quote_border.width.max(1),
                        line.bounds.size.height,
                    ),
                ),
                page_offset,
            ),
            style: style.block_quote_border,
        });
    }
    if code_surface {
        page.push_command(DisplayCommand::Fill {
            bounds: translate(line.bounds, page_offset),
            style: if line.wrapped {
                style.code_continuation_background
            } else {
                style.code_background
            },
        });
    }
    if kind == LayoutBlockKind::Table {
        add_table_decoration(page, line, page_offset, style, table, line_index);
    }
    for fragment in &line.fragments {
        let bounds = translate(fragment.bounds, page_offset);
        if let Some(image) = &fragment.image {
            page.push_command(DisplayCommand::Image {
                bounds,
                image: image.image.clone(),
                source: image.source.clone(),
                alt: image.alt.clone(),
            });
            if let Some(target) = &fragment.link {
                page.add_hit_region(HitRegion::new(bounds, target.clone()));
            }
            continue;
        }
        if fragment.style.code && !code_surface {
            page.push_command(DisplayCommand::Fill {
                bounds,
                style: style.inline_code_background,
            });
        }
        page.push_command(DisplayCommand::Text {
            bounds,
            text: fragment.text.clone(),
            style: fragment.style,
        });
        if fragment.style.strikethrough && bounds.size.width > 0 {
            page.push_command(DisplayCommand::Rule {
                bounds: Rect::new(
                    Point::new(
                        bounds.top_left.x,
                        bounds
                            .top_left
                            .y
                            .saturating_add((bounds.size.height / 2) as i32),
                    ),
                    embedded_graphics::geometry::Size::new(bounds.size.width, 1),
                ),
                style: BorderStyle::new(Color::BLACK, 1),
            });
        }
        if let Some(target) = &fragment.link {
            page.add_hit_region(HitRegion::new(bounds, target.clone()));
        }
    }
    if line.fragments.is_empty() && !code_surface {
        let bounds = translate(line.bounds, page_offset);
        match kind {
            LayoutBlockKind::Rule => page.push_command(DisplayCommand::Rule {
                bounds,
                style: style.thematic_break,
            }),
            LayoutBlockKind::Image => page.push_command(DisplayCommand::ImagePlaceholder {
                bounds,
                source: None,
                alt: String::new(),
            }),
            // The code surface fill is sufficient for a source-newline line.
            // Do not turn an empty code line into the generic fallback rule.
            LayoutBlockKind::Code => {}
            _ => page.push_command(DisplayCommand::Rule {
                bounds,
                style: BorderStyle::new(Color::BLACK, 1),
            }),
        }
    }
}

fn table_row_at(table: &TableLayout, line_index: usize) -> Option<&TableRowLayout> {
    table
        .rows
        .iter()
        .find(|row| row.line_range.contains(&line_index))
}

fn table_header_for_line(table: &TableLayout, line_index: usize) -> Option<&TableRowLayout> {
    let row = table_row_at(table, line_index)?;
    table
        .rows
        .iter()
        .find(|candidate| candidate.group == row.group && candidate.header)
}

fn table_row_height(block: &crate::layout::LayoutBlock, row: &TableRowLayout) -> i32 {
    let Some(first) = block.lines.get(row.line_range.start) else {
        return 0;
    };
    let last = block
        .lines
        .get(row.line_range.end.saturating_sub(1))
        .unwrap_or(first);
    line_bottom(last.bounds).saturating_sub(first.bounds.top_left.y)
}

fn page_origin_and_header(
    layout: &DocumentLayout,
    page: &PageLayout,
    style: &ReaderStyle,
) -> (i32, Option<(usize, std::ops::Range<usize>)>) {
    let Some(block) = layout.blocks().get(page.range.start.block) else {
        return (0, None);
    };
    let Some(line) = block.lines.get(page.range.start.line) else {
        return (0, None);
    };

    let mut origin_y = if page.number == 1 {
        0
    } else {
        line.bounds
            .top_left
            .y
            .saturating_sub(style.page_padding.top as i32)
    };
    let mut repeated_header = None;
    if block.kind == LayoutBlockKind::Table {
        if let Some(table) = block.table.as_ref() {
            if let Some(row) = table_row_at(table, page.range.start.line) {
                if !row.header {
                    if let Some(header) = table_header_for_line(table, page.range.start.line) {
                        origin_y = origin_y.saturating_sub(table_row_height(block, header));
                        repeated_header = Some((page.range.start.block, header.line_range.clone()));
                    }
                }
            }
        }
    }
    (origin_y, repeated_header)
}

fn add_table_decoration(
    page: &mut PageLayout,
    line: &LayoutLine,
    page_offset: Point,
    style: &ReaderStyle,
    table: Option<&TableLayout>,
    line_index: usize,
) {
    let Some(table) = table else {
        return;
    };
    let Some(row) = table_row_at(table, line_index) else {
        return;
    };
    let Some(group) = table.groups.get(row.group) else {
        return;
    };
    let border_width = style.table_border.width.max(1);
    let border_style = if row.header {
        style.table_header_border
    } else {
        style.table_border
    };
    let mut cell_x = group.x;
    let multiline = row.line_range.len() > 1;
    for (column_index, column_width) in group.widths.iter().enumerate() {
        let remaining = group
            .width
            .saturating_sub(cell_x.saturating_sub(group.x) as u32);
        let cell_width = if column_index + 1 == group.widths.len() {
            remaining
        } else {
            column_width
                .saturating_add(style.table_cell_padding.saturating_mul(2))
                .saturating_add(border_width)
        };
        let bounds = translate(
            Rect::new(
                Point::new(cell_x, line.bounds.top_left.y),
                embedded_graphics::geometry::Size::new(cell_width, line.bounds.size.height),
            ),
            page_offset,
        );
        if row.header {
            page.push_command(DisplayCommand::Fill {
                bounds,
                style: style.table_header_fill,
            });
        }
        if multiline {
            let line_width = border_style.width.max(1).min(cell_width);
            let line_height = line.bounds.size.height;
            let left = Rect::new(
                Point::new(cell_x, line.bounds.top_left.y),
                embedded_graphics::geometry::Size::new(line_width, line_height),
            );
            let right = Rect::new(
                Point::new(
                    cell_x.saturating_add(cell_width.saturating_sub(line_width) as i32),
                    line.bounds.top_left.y,
                ),
                embedded_graphics::geometry::Size::new(line_width, line_height),
            );
            page.push_command(DisplayCommand::Rule {
                bounds: translate(left, page_offset),
                style: border_style,
            });
            page.push_command(DisplayCommand::Rule {
                bounds: translate(right, page_offset),
                style: border_style,
            });
        } else {
            page.push_command(DisplayCommand::Border {
                bounds,
                style: border_style,
            });
        }
        cell_x = cell_x.saturating_add(cell_width as i32);
    }

    if multiline {
        let horizontal_height = border_style.width.max(1);
        let y = if line_index == row.line_range.start {
            line.bounds.top_left.y
        } else if line_index + 1 == row.line_range.end {
            line_bottom(line.bounds).saturating_sub(horizontal_height as i32)
        } else {
            return;
        };
        page.push_command(DisplayCommand::Rule {
            bounds: translate(
                Rect::new(
                    Point::new(group.x, y),
                    embedded_graphics::geometry::Size::new(group.width, horizontal_height),
                ),
                page_offset,
            ),
            style: border_style,
        });
    }
}

fn line_bottom(rectangle: Rect) -> i32 {
    rectangle
        .top_left
        .y
        .saturating_add(rectangle.size.height.min(i32::MAX as u32) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Block, Document, Inline, ListItem, Table};
    use crate::geometry::Viewport;
    use crate::layout::{LayoutBlock, LayoutBlockKind, LayoutEngine, LayoutLine};
    use crate::style::Insets;
    use embedded_graphics::geometry::Size;

    fn page() -> PageLayout {
        PageLayout::new(1, Viewport::new(100, 80))
    }

    #[test]
    fn commands_are_translated_and_clipped_to_page_space() {
        let mut page = page();
        page.push_command(DisplayCommand::Fill {
            bounds: Rect::new(Point::new(-4, 6), Size::new(12, 10)),
            style: FillStyle::new(Color::WHITE),
        });

        assert_eq!(
            page.display_list()[0].bounds(),
            Rect::new(Point::new(0, 6), Size::new(8, 10))
        );
        assert_eq!(
            translate(
                Rect::new(Point::new(8, 40), Size::new(10, 10)),
                Point::new(3, -20)
            ),
            Rect::new(Point::new(11, 20), Size::new(10, 10))
        );
    }

    #[test]
    fn one_target_can_have_multiple_hit_rectangles() {
        let mut page = page();
        let target = NavigationTarget::from_destination("chapter.md#intro");
        page.add_hit_region(HitRegion::new(
            Rect::new(Point::new(5, 5), Size::new(30, 10)),
            target.clone(),
        ));
        page.add_hit_region(HitRegion::new(
            Rect::new(Point::new(5, 25), Size::new(25, 10)),
            target.clone(),
        ));

        assert_eq!(page.hit_regions.len(), 2);
        assert_eq!(page.hit_test(Point::new(10, 28)).unwrap().target, target);
    }

    #[test]
    fn overlapping_regions_use_the_last_added_region() {
        let mut page = page();
        page.add_hit_region(HitRegion::new(
            Rect::new(Point::new(10, 10), Size::new(30, 30)),
            NavigationTarget::Anchor("underlay".into()),
        ));
        page.add_hit_region(HitRegion::new(
            Rect::new(Point::new(20, 20), Size::new(30, 30)),
            NavigationTarget::Anchor("overlay".into()),
        ));

        assert_eq!(
            page.hit_test(Point::new(25, 25)).unwrap().target,
            NavigationTarget::Anchor("overlay".into())
        );
        assert!(page.hit_test(Point::new(90, 70)).is_none());
    }

    #[test]
    fn synthetic_page_contains_all_supported_primitives() {
        let mut page = page();
        let bounds = Rect::new(Point::new(4, 4), Size::new(80, 50));
        page.push_command(DisplayCommand::Fill {
            bounds,
            style: FillStyle::new(Color::rgba(245, 245, 245, 255)),
        });
        page.push_command(DisplayCommand::Border {
            bounds,
            style: BorderStyle::new(Color::BLACK, 2),
        });
        page.push_command(DisplayCommand::Text {
            bounds: Rect::new(Point::new(8, 8), Size::new(72, 12)),
            text: "A synthetic page".into(),
            style: TextStyle::new(12, 16),
        });
        page.push_command(DisplayCommand::Rule {
            bounds: Rect::new(Point::new(8, 26), Size::new(72, 2)),
            style: BorderStyle::new(Color::BLACK, 1),
        });
        page.push_command(DisplayCommand::ImagePlaceholder {
            bounds: Rect::new(Point::new(8, 32), Size::new(40, 18)),
            source: Some("images/diagram.png".into()),
            alt: "diagram".into(),
        });

        assert_eq!(page.display_list().len(), 5);
        assert_eq!(page.display_list()[0].bounds(), bounds);
        assert_eq!(page.display_list()[4].bounds().size, Size::new(40, 18));
    }

    #[test]
    fn paginator_uses_layout_style_for_code_quotes_and_rules() {
        let style = ReaderStyle {
            page_padding: crate::style::Insets::all(2),
            inline_code_background: FillStyle::new(Color::rgb(1, 2, 3)),
            code_background: FillStyle::new(Color::rgb(4, 5, 6)),
            block_quote_border: BorderStyle::new(Color::rgb(7, 8, 9), 3),
            thematic_break: BorderStyle::new(Color::rgb(10, 11, 12), 2),
            ..ReaderStyle::default()
        };
        let document = crate::Document::from_blocks(vec![
            crate::Block::Paragraph(vec![crate::Inline::Code("inline".into())]),
            crate::Block::CodeBlock {
                language: None,
                info: None,
                code: "block".into(),
            },
            crate::Block::Quote(vec![crate::Block::paragraph("quote")]),
            crate::Block::Rule,
        ]);
        let layout = crate::LayoutEngine::new(style).layout(&document, Viewport::new(200, 200));
        let page = Paginator::new(style).paginate(&layout).remove(0);

        assert!(page.commands.iter().any(|command| {
            matches!(
                command,
                DisplayCommand::Fill { style, .. } if style.color == Color::rgb(1, 2, 3)
            )
        }));
        assert!(page.commands.iter().any(|command| {
            matches!(
                command,
                DisplayCommand::Fill { style, .. } if style.color == Color::rgb(4, 5, 6)
            )
        }));
        assert!(page.commands.iter().any(|command| {
            matches!(
                command,
                DisplayCommand::Rule { style, .. } if style.color == Color::rgb(7, 8, 9)
            )
        }));
        assert!(page.commands.iter().any(|command| {
            matches!(
                command,
                DisplayCommand::Rule { style, .. } if style.color == Color::rgb(10, 11, 12)
            )
        }));
    }

    #[test]
    fn code_surface_fills_blank_and_wrapped_lines_without_fallback_rules() {
        let style = ReaderStyle {
            code_block_padding: 4,
            code_background: FillStyle::new(Color::rgb(240, 240, 240)),
            code_continuation_background: FillStyle::new(Color::rgb(236, 236, 236)),
            ..pagination_style()
        };
        let document = Document::from_blocks(vec![Block::CodeBlock {
            language: None,
            info: None,
            code: "short\n\nThis deliberately long source line wraps inside the padded code surface and keeps every continuation aligned".into(),
        }]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(58, 500));
        let code_block = &layout.blocks()[0];
        let page = Paginator::new(style).paginate(&layout).remove(0);
        let fills = page
            .display_list()
            .iter()
            .filter_map(|command| match command {
                DisplayCommand::Fill { bounds, style } => Some((*bounds, *style)),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(fills.len(), code_block.lines.len());
        assert!(fills.iter().all(|(bounds, _)| {
            bounds.top_left.x == style.page_padding.left as i32
                && bounds.size.width == 58 - style.page_padding.left - style.page_padding.right
        }));
        assert!(fills
            .iter()
            .any(|(_, fill)| *fill == style.code_continuation_background));
        assert!(!page
            .display_list()
            .iter()
            .any(|command| matches!(command, DisplayCommand::Rule { .. })));
    }

    #[test]
    fn nested_code_surface_keeps_blank_and_text_lines_decorated() {
        let style = pagination_style();
        let document = crate::parse::parse("> ```rust\n> \n> let value = 1;\n> ```\n")
            .expect("nested code fence should parse");
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(100, 100));
        let quote = &layout.blocks()[0];
        assert_eq!(quote.kind, LayoutBlockKind::Quote);
        assert!(quote.lines.iter().all(|line| line.code));
        assert!(quote.lines.iter().any(|line| line.fragments.is_empty()));

        let page = Paginator::new(style).paginate(&layout).remove(0);
        let fills = page
            .display_list()
            .iter()
            .filter_map(|command| match command {
                DisplayCommand::Fill { bounds, style } => Some((*bounds, *style)),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(fills.len(), quote.lines.len());
        assert!(fills.iter().zip(&quote.lines).all(|((bounds, _), line)| {
            bounds.top_left == line.bounds.top_left && bounds.size == line.bounds.size
        }));
        assert!(!page.display_list().iter().any(|command| {
            matches!(
                command,
                DisplayCommand::Rule { style, .. }
                    if *style == BorderStyle::new(Color::BLACK, 1)
            )
        }));
    }

    #[test]
    fn strikethrough_fragments_get_a_visible_decoration() {
        let style = pagination_style();
        let document =
            Document::from_blocks(vec![Block::Paragraph(vec![Inline::Strikethrough(vec![
                Inline::Text("retired".into()),
            ])])]);
        let layout = crate::LayoutEngine::new(style).layout(&document, Viewport::new(200, 40));
        let page = Paginator::new(style).paginate(&layout).remove(0);

        assert!(page.commands.iter().any(|command| {
            matches!(command, DisplayCommand::Text { style, .. } if style.strikethrough)
        }));
        assert!(page.commands.iter().any(|command| {
            matches!(command, DisplayCommand::Rule { bounds, .. } if bounds.size.height == 1)
        }));
    }

    #[test]
    fn paginator_translates_each_page_to_viewport_coordinates() {
        let viewport = Viewport::new(100, 30);
        let style = ReaderStyle {
            page_padding: crate::style::Insets {
                top: 4,
                right: 4,
                bottom: 4,
                left: 4,
            },
            ..ReaderStyle::default()
        };
        let layout = DocumentLayout {
            viewport,
            blocks: vec![
                LayoutBlock {
                    kind: LayoutBlockKind::Paragraph,
                    bounds: Rect::new(Point::new(4, 4), Size::new(40, 10)),
                    lines: vec![LayoutLine {
                        bounds: Rect::new(Point::new(4, 4), Size::new(40, 10)),
                        fragments: vec![crate::layout::LayoutFragment {
                            text: "first".into(),
                            bounds: Rect::new(Point::new(4, 4), Size::new(25, 10)),
                            style: style.body,
                            image: None,
                            link: None,
                        }],
                        code: false,
                        wrapped: false,
                    }],
                    anchor: None,
                    table: None,
                },
                LayoutBlock {
                    kind: LayoutBlockKind::Paragraph,
                    bounds: Rect::new(Point::new(4, 34), Size::new(40, 10)),
                    lines: vec![LayoutLine {
                        bounds: Rect::new(Point::new(4, 34), Size::new(40, 10)),
                        fragments: vec![crate::layout::LayoutFragment {
                            text: "second".into(),
                            bounds: Rect::new(Point::new(4, 34), Size::new(30, 10)),
                            style: style.body,
                            image: None,
                            link: None,
                        }],
                        code: false,
                        wrapped: false,
                    }],
                    anchor: None,
                    table: None,
                },
            ],
        };

        let pages = Paginator::new(style).paginate(&layout);

        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].display_list()[0].bounds().top_left.y, 4);
        assert_eq!(pages[1].display_list()[0].bounds().top_left.y, 4);
    }

    fn pagination_style() -> ReaderStyle {
        ReaderStyle {
            page_padding: Insets::all(0),
            body: TextStyle::new(10, 10),
            heading: TextStyle {
                bold: true,
                ..TextStyle::new(10, 10)
            },
            code: TextStyle::new(10, 10),
            paragraph_spacing: 0,
            heading_spacing_before: 0,
            heading_spacing_after: 0,
            list_item_spacing: 2,
            ..ReaderStyle::default()
        }
    }

    fn text(block: &LayoutBlock) -> String {
        block
            .lines
            .iter()
            .flat_map(|line| line.fragments.iter().map(|fragment| fragment.text.as_str()))
            .collect()
    }

    #[test]
    fn exact_fit_stays_on_page_and_one_line_overflow_starts_next_page() {
        let style = pagination_style();
        let document = Document::from_blocks(vec![
            Block::paragraph("first"),
            Block::paragraph("second"),
            Block::paragraph("third"),
        ]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(200, 20));
        let pages = Paginator::new(style).paginate(&layout);

        assert_eq!(pages.len(), 2);
        assert_eq!(
            pages[0].range,
            DocumentRange::new(DocumentCursor::new(0, 0), DocumentCursor::new(2, 0),)
        );
        assert_eq!(
            pages[1].range,
            DocumentRange::new(DocumentCursor::new(2, 0), DocumentCursor::new(3, 0),)
        );
        assert_eq!(pages[0].display_list()[1].bounds().top_left.y, 10);
        assert_eq!(pages[1].display_list()[0].bounds().top_left.y, 0);
    }

    #[test]
    fn heading_near_bottom_moves_with_following_content() {
        let style = pagination_style();
        let document = Document::from_blocks(vec![
            Block::paragraph("intro"),
            Block::heading(1, "section"),
            Block::paragraph("body"),
        ]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(200, 20));
        let pages = Paginator::new(style).paginate(&layout);

        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].range.end, DocumentCursor::new(1, 0));
        assert_eq!(pages[1].range.start, DocumentCursor::new(1, 0));
        assert_eq!(pages[1].display_list()[0].bounds().top_left.y, 0);
        assert_eq!(pages[1].display_list()[1].bounds().top_left.y, 10);
    }

    #[test]
    fn lists_split_at_line_boundaries_without_losing_nested_content() {
        let style = pagination_style();
        let mut nested = ListItem::new(vec![Inline::Text("nested item".into())]);
        nested.children.push(Block::paragraph("nested detail"));
        let document = Document::from_blocks(vec![Block::List {
            ordered: false,
            items: vec![
                ListItem::new(vec![Inline::Text("first item".into())]),
                {
                    let mut item = ListItem::new(vec![Inline::Text("second item".into())]);
                    item.children.push(Block::List {
                        ordered: false,
                        items: vec![nested],
                    });
                    item
                },
                ListItem::new(vec![Inline::Text("third item".into())]),
            ],
        }]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(200, 24));
        let pages = Paginator::new(style).paginate(&layout);

        let rendered: String = pages
            .iter()
            .flat_map(|page| page.display_list().iter())
            .filter_map(|command| match command {
                DisplayCommand::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let laid_out = text(&layout.blocks()[0]);
        assert!(pages.len() > 1);
        assert!(rendered.contains("first item"));
        assert!(rendered.contains("second item"));
        assert!(rendered.contains("nested item"));
        assert!(rendered.contains("nested detail"));
        assert!(rendered.contains("third item"));
        assert_eq!(rendered.replace(' ', ""), laid_out.replace(' ', ""));
    }

    #[test]
    fn long_paragraph_uses_line_ranges_and_repeated_pagination_is_identical() {
        let style = pagination_style();
        let document = Document::from_blocks(vec![Block::paragraph(
            "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen",
        )]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(45, 24));
        let paginator = Paginator::new(style);
        let first = paginator.paginate(&layout);
        let second = paginator.paginate(&layout);

        assert_eq!(first, second);
        assert!(first.len() > 1);
        assert_eq!(first[0].range.start, DocumentCursor::new(0, 0));
        assert_eq!(first.last().unwrap().range.end, DocumentCursor::new(1, 0));
        for pair in first.windows(2) {
            assert_eq!(pair[0].range.end, pair[1].range.start);
        }
    }

    #[test]
    fn pagination_navigation_round_trips_by_logical_page() {
        let style = pagination_style();
        let document = Document::from_blocks(vec![
            Block::paragraph("one"),
            Block::paragraph("two"),
            Block::paragraph("three"),
        ]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(200, 10));
        let pages = Paginator::new(style).paginate(&layout);
        assert_eq!(pages.page_index_for_cursor(pages[1].range.start), Some(1));
        let next = pages.next_page(0).expect("next page");
        let previous = pages.previous_page(next.number - 1).expect("previous page");
        assert_eq!(previous.range, pages[0].range);
        assert_eq!(pages.page(usize::MAX), None);
    }

    #[test]
    fn compact_index_rebuilds_table_pages_identically() {
        let document = crate::parse::parse(include_str!("../tests/fixtures/tables-code-images.md"))
            .expect("table fixture should parse");
        let style = ReaderStyle::default();
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(180, 80));
        let paginator = Paginator::new(style);
        let eager = paginator.paginate(&layout);
        let index = paginator.index(&layout);

        assert_eq!(index.page_count(), eager.page_count());
        for page_index in 0..eager.page_count() {
            assert_eq!(
                paginator.page_from_index(&layout, &index, page_index),
                eager.page(page_index).cloned()
            );
        }
    }

    #[test]
    fn empty_document_has_one_empty_logical_page() {
        let style = pagination_style();
        let layout = LayoutEngine::new(style).layout(&Document::new(), Viewport::new(100, 20));
        let pages = Paginator::new(style).paginate(&layout);

        assert_eq!(pages.len(), 1);
        assert_eq!(
            pages[0].range,
            DocumentRange::empty(DocumentCursor::new(0, 0))
        );
        assert!(pages[0].display_list().is_empty());
    }

    #[test]
    fn rules_remain_atomic_while_quotes_and_code_split_by_line() {
        let style = pagination_style();
        let document = Document::from_blocks(vec![
            Block::paragraph("before"),
            Block::Rule,
            Block::Quote(vec![Block::Paragraph(vec![
                Inline::Text("one".into()),
                Inline::HardBreak,
                Inline::Text("two".into()),
                Inline::HardBreak,
                Inline::Text("three".into()),
            ])]),
            Block::CodeBlock {
                language: None,
                info: None,
                code: "a\nb\nc".into(),
            },
        ]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(200, 15));
        let pages = Paginator::new(style).paginate(&layout);

        assert!(pages.len() >= 4);
        assert!(matches!(
            pages[1].display_list().first(),
            Some(DisplayCommand::Rule { style, .. })
                if *style == pagination_style().thematic_break
        ));
        assert!(pages.iter().any(|page| {
            page.display_list().iter().any(|command| {
                matches!(command, DisplayCommand::Fill { style, .. }
                    if *style == pagination_style().code_background)
            })
        }));
        assert!(pages.iter().any(|page| {
            page.display_list().iter().any(|command| {
                matches!(command, DisplayCommand::Rule { style, .. }
                    if *style == pagination_style().block_quote_border)
            })
        }));
    }

    #[test]
    fn tables_break_between_rows_and_repeat_headers_on_continuations() {
        let style = pagination_style();
        let document = Document::from_blocks(vec![Block::Table(crate::Table {
            headers: vec![vec![Inline::Text("Header".into())]],
            rows: (0..6)
                .map(|index| vec![vec![Inline::Text(format!("row-{index}"))]])
                .collect(),
            alignments: Vec::new(),
        })]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(100, 25));
        let table = layout.blocks()[0].table.as_ref().unwrap();
        let pages = Paginator::new(style).paginate(&layout);

        assert!(pages.len() > 1);
        assert!(pages.iter().skip(1).all(|page| page
            .display_list()
            .iter()
            .filter_map(|command| match command {
                DisplayCommand::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .any(|text| text == "Header")));
        for pair in pages.windows(2) {
            assert_eq!(pair[0].range.end, pair[1].range.start);
        }
        assert!(pages
            .iter()
            .flat_map(|page| page.display_list())
            .any(|command| matches!(command, DisplayCommand::Border { .. })));
        assert!(table.rows.iter().all(|row| !row.line_range.is_empty()));
        assert!(pages.iter().all(|page| {
            page.display_list()
                .iter()
                .filter_map(|command| match command {
                    DisplayCommand::Text { bounds, .. } => Some(bounds.top_left.y),
                    _ => None,
                })
                .all(|y| y >= style.page_padding.top as i32)
        }));
    }

    #[test]
    fn multiline_table_rows_have_only_outer_horizontal_boundaries() {
        let style = pagination_style();
        let document = Document::from_blocks(vec![Block::Table(Table {
            headers: vec![
                vec![Inline::Text("Key".into())],
                vec![Inline::Text("Detail".into())],
            ],
            rows: vec![vec![
                vec![Inline::Text("row".into())],
                vec![Inline::Text(
                    "A long detail cell wraps across several display lines without changing the logical row."
                        .into(),
                )],
            ]],
            alignments: Vec::new(),
        })]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(120, 300));
        let table_block = &layout.blocks()[0];
        let table = table_block.table.as_ref().expect("table metadata");
        let row = table
            .rows
            .iter()
            .find(|row| !row.header)
            .expect("data row metadata");
        assert!(row.line_range.len() > 1);
        let group = &table.groups[row.group];
        let pages = Paginator::new(style).paginate(&layout);
        assert_eq!(pages.len(), 1);

        let horizontal_rules = pages[0]
            .display_list()
            .iter()
            .filter_map(|command| match command {
                DisplayCommand::Rule {
                    bounds,
                    style: rule_style,
                } if *rule_style == style.table_border && bounds.size.width == group.width => {
                    Some(*bounds)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let row_start = table_block.lines[row.line_range.start].bounds.top_left.y;
        let row_end = line_bottom(table_block.lines[row.line_range.end.saturating_sub(1)].bounds)
            .saturating_sub(style.table_border.width.max(1) as i32);

        assert_eq!(horizontal_rules.len(), 2);
        assert_eq!(
            horizontal_rules
                .iter()
                .map(|bounds| bounds.top_left.y)
                .collect::<Vec<_>>(),
            vec![row_start, row_end]
        );
    }

    #[test]
    fn grouped_multiline_rows_have_only_outer_horizontal_boundaries() {
        let style = pagination_style();
        let document = Document::from_blocks(vec![Block::Table(Table {
            headers: vec![
                vec![Inline::Text("Key".into())],
                vec![Inline::Text("State".into())],
                vec![Inline::Text("Owner".into())],
                vec![Inline::Text("Detail".into())],
            ],
            rows: vec![vec![
                vec![Inline::Text("row".into())],
                vec![Inline::Text("ready".into())],
                vec![Inline::Text("reader".into())],
                vec![Inline::Text(
                    "A long grouped continuation cell wraps across several display lines.".into(),
                )],
            ]],
            alignments: Vec::new(),
        })]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(50, 600));
        let table_block = &layout.blocks()[0];
        let table = table_block.table.as_ref().expect("table metadata");
        assert_eq!(table.mode, crate::layout::TableLayoutMode::Grouped);
        assert!(table.groups.len() > 1);
        assert!(table
            .rows
            .iter()
            .any(|row| !row.header && row.line_range.len() > 1));

        let pages = Paginator::new(style).paginate(&layout);
        assert_eq!(pages.len(), 1);
        let expected = table
            .rows
            .iter()
            .filter(|row| !row.header && row.line_range.len() > 1)
            .flat_map(|row| {
                let group = &table.groups[row.group];
                let start = table_block.lines[row.line_range.start].bounds.top_left.y;
                let end =
                    line_bottom(table_block.lines[row.line_range.end.saturating_sub(1)].bounds)
                        .saturating_sub(style.table_border.width.max(1) as i32);
                [(group.width, start), (group.width, end)]
            })
            .collect::<Vec<_>>();
        let mut actual = pages[0]
            .display_list()
            .iter()
            .filter_map(|command| match command {
                DisplayCommand::Rule {
                    bounds,
                    style: rule_style,
                } if *rule_style == style.table_border
                    && table
                        .groups
                        .iter()
                        .any(|group| group.width == bounds.size.width) =>
                {
                    Some((bounds.size.width, bounds.top_left.y))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut expected = expected;
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected);
    }

    #[test]
    fn wrapped_table_headers_use_header_border_width_for_vertical_rules() {
        let style = pagination_style();
        let document = Document::from_blocks(vec![Block::Table(crate::Table {
            headers: vec![vec![Inline::Text(
                "wrapped table header content has multiple displayed lines".into(),
            )]],
            rows: Vec::new(),
            alignments: Vec::new(),
        })]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(42, 400));
        let table = layout.blocks()[0].table.as_ref().unwrap();
        assert!(table.rows[0].header);
        assert!(table.rows[0].line_range.len() > 1);

        let page = Paginator::new(style).paginate(&layout).remove(0);
        let vertical_header_rules = page
            .display_list()
            .iter()
            .filter_map(|command| match command {
                DisplayCommand::Rule {
                    bounds,
                    style: rule_style,
                } if *rule_style == style.table_header_border
                    && bounds.size.height > bounds.size.width =>
                {
                    Some(bounds.size.width)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!vertical_header_rules.is_empty());
        assert!(vertical_header_rules
            .iter()
            .all(|width| *width == style.table_header_border.width));
    }

    #[test]
    fn oversized_table_rows_split_deterministically_after_first_placement() {
        let style = pagination_style();
        let document = Document::from_blocks(vec![Block::Table(crate::Table {
            headers: vec![vec![Inline::Text("Key".into())]],
            rows: vec![vec![vec![Inline::Text(
                "a-very-long-unbreakable-identifier-that-needs-many-lines".into(),
            )]]],
            alignments: Vec::new(),
        })]);
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(24, 44));
        let first = Paginator::new(style).paginate(&layout);
        let second = Paginator::new(style).paginate(&layout);

        assert!(first.len() > 1);
        assert_eq!(first, second);
        assert!(first
            .iter()
            .skip(1)
            .all(|page| page.display_list().iter().any(
                |command| matches!(command, DisplayCommand::Text { text, .. } if text == "Key")
            )));

        let table = layout.blocks()[0].table.as_ref().expect("table metadata");
        let row = table
            .rows
            .iter()
            .find(|row| !row.header)
            .expect("oversized row metadata");
        let group = &table.groups[row.group];
        let horizontal_rules = first
            .iter()
            .flat_map(|page| page.display_list())
            .filter(|command| {
                matches!(
                    command,
                    DisplayCommand::Rule { bounds, style: rule_style }
                        if *rule_style == style.table_border && bounds.size.width == group.width
                )
            })
            .count();
        assert_eq!(horizontal_rules, 2);
        assert!(first.iter().skip(1).all(|page| {
            page.display_list().iter().any(|command| {
                matches!(
                    command,
                    DisplayCommand::Rule { bounds, style: rule_style }
                        if *rule_style == style.table_border
                            && bounds.size.width == style.table_border.width.max(1)
                )
            })
        }));
    }
}
