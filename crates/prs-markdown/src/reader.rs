//! High-level reader state, kept separate from physical input and display IO.

use crate::document::{image_fallback, Document};
use crate::geometry::Viewport;
use crate::image::ImageResources;
use crate::layout::{ApproximateTextMeasurer, DocumentLayout, LayoutEngine, TextMeasurer};
use crate::navigation::{DocumentId, DocumentLocation, NavigationTarget, ReaderHistory};
use crate::pagination::{
    DocumentCursor, DocumentRange, HitRegion, PageLayout, Pagination, PaginationIndex, Paginator,
};
use crate::parse::{ComrakParser, MarkdownParser, ParseError};
use crate::render::EmbeddedGraphicsRenderer;
use crate::resources::{ResourceError, ResourceProvider, ResourceTarget};
use crate::style::ReaderStyle;
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::Point;
use embedded_graphics::pixelcolor::Rgb888;
use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReaderAction {
    NextPage,
    PreviousPage,
    Back,
    Forward,
}

/// A device-independent result of a reader operation.
///
/// `ExternalUrl` is deliberately an event rather than an operation: the
/// library never opens a browser, starts Android, or chooses another handler.
/// Page indices in these events are zero-based, matching [`ReaderSession`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReaderEvent {
    Opened {
        location: DocumentLocation,
        page_count: usize,
    },
    PageChanged {
        page: usize,
        page_count: usize,
    },
    Navigated {
        location: DocumentLocation,
        page: usize,
        page_count: usize,
    },
    Back {
        location: DocumentLocation,
        page: usize,
        page_count: usize,
    },
    Forward {
        location: DocumentLocation,
        page: usize,
        page_count: usize,
    },
    ExternalUrl(String),
    Asset(PathBuf),
    NoAction,
}

/// Errors raised while loading or navigating a reading session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReaderError {
    NoDocumentOpen,
    Resource(ResourceError),
    Parse(ParseError),
    AnchorNotFound { document: PathBuf, anchor: String },
    UnsupportedTarget(ResourceTarget),
}

impl fmt::Display for ReaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDocumentOpen => formatter.write_str("no Markdown document is open"),
            Self::Resource(error) => error.fmt(formatter),
            Self::Parse(error) => error.fmt(formatter),
            Self::AnchorNotFound { document, anchor } => write!(
                formatter,
                "anchor '{anchor}' was not found in Markdown document '{}'",
                document.display()
            ),
            Self::UnsupportedTarget(target) => {
                write!(formatter, "reader cannot navigate to target {target:?}")
            }
        }
    }
}

impl Error for ReaderError {}

impl From<ResourceError> for ReaderError {
    fn from(error: ResourceError) -> Self {
        Self::Resource(error)
    }
}

impl From<ParseError> for ReaderError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

/// A stable position inside one top-level document block.
///
/// The offset counts Unicode scalar values in the block's stable reading text.
/// It identifies content rather than a wrapped layout line, so it remains
/// meaningful when the viewport or typography changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ContentAnchor {
    pub block: usize,
    pub offset: usize,
}

impl ContentAnchor {
    pub const fn new(block: usize, offset: usize) -> Self {
        Self { block, offset }
    }
}

/// A location retained in reader history, including its physical page cursor
/// and stable content anchor.
///
/// Keeping the cursor alongside the document location is what makes Back
/// return to the place where a link was activated rather than to page zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadingLocation {
    pub location: DocumentLocation,
    pub page: usize,
    pub cursor: DocumentCursor,
    pub content: ContentAnchor,
}

impl ReadingLocation {
    pub fn document(&self) -> &DocumentId {
        &self.location.document
    }

    pub fn anchor(&self) -> Option<&str> {
        self.location.anchor.as_deref()
    }

    pub fn content_anchor(&self) -> ContentAnchor {
        self.content
    }
}

/// Rendering errors distinguish reader state errors from draw-target errors.
#[derive(Debug)]
pub enum ReaderRenderError<E> {
    Reader(ReaderError),
    Target(E),
}

impl<E: fmt::Display> fmt::Display for ReaderRenderError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reader(error) => error.fmt(formatter),
            Self::Target(error) => error.fmt(formatter),
        }
    }
}

/// Explicit steady-state resource policy for the high-level reader.
///
/// `Reader::with_components` uses these bounds. The compatibility
/// `Reader::new` constructor keeps its historical eager-pagination behaviour;
/// T1 and new callers should use the component constructor so old page and
/// host-inspection APIs remain source-compatible during the transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReaderLimits {
    pub max_history_entries: usize,
    pub max_cached_documents: usize,
    pub page_cache_capacity: usize,
    pub image_retained_bytes: usize,
    pub image_entry_capacity: usize,
}

impl Default for ReaderLimits {
    fn default() -> Self {
        Self {
            max_history_entries: 16,
            max_cached_documents: 2,
            page_cache_capacity: 3,
            image_retained_bytes: crate::image::DEFAULT_RETAINED_BYTES,
            image_entry_capacity: crate::image::DEFAULT_IMAGE_ENTRIES,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReaderCacheStats {
    pub history_entries: usize,
    pub history_limit: usize,
    pub cached_documents: usize,
    pub document_cache_limit: usize,
    pub cached_pages: usize,
    pub page_cache_limit: usize,
    pub image_retained_bytes: usize,
    pub image_retained_limit: usize,
    pub image_entries: usize,
    pub image_entry_limit: usize,
}

/// A complete, host-configurable Markdown reading session.
///
/// The reader coordinates resource loading, parsing, layout, pagination, and
/// navigation. It owns the current document and derived page state. A caller
/// supplies a [`TextMeasurer`] for layout; rendering accepts a separate caller
/// supplied [`crate::typography::TextEngine`] and draw target so no font or
/// physical display policy is embedded in this controller.
pub struct Reader<P, M = ApproximateTextMeasurer, Parser = ComrakParser>
where
    P: ResourceProvider,
    M: TextMeasurer + Clone,
    Parser: MarkdownParser,
{
    provider: P,
    measurer: M,
    parser: Parser,
    style: ReaderStyle,
    viewport: Viewport,
    current: Option<OpenDocument>,
    document_cache: DocumentCache,
    history: Vec<ReadingLocation>,
    history_index: usize,
    limits: ReaderLimits,
    eager_pagination: bool,
}

struct OpenDocument {
    location: DocumentLocation,
    document: Document,
    layout: DocumentLayout,
    pagination: Option<Pagination>,
    page_index: Option<PaginationIndex>,
    page_cache: PageCache,
    images: ImageResources,
    page: usize,
    cursor: DocumentCursor,
}

impl OpenDocument {
    fn page_count(&self) -> usize {
        self.pagination.as_ref().map_or_else(
            || {
                self.page_index
                    .as_ref()
                    .map_or(0, PaginationIndex::page_count)
            },
            Pagination::page_count,
        )
    }

    fn page(&self) -> Option<&PageLayout> {
        if let Some(pagination) = &self.pagination {
            pagination.page(self.page)
        } else {
            self.page_cache.get(self.page)
        }
    }

    fn page_range(&self, index: usize) -> DocumentRange {
        self.pagination
            .as_ref()
            .and_then(|pages| pages.page(index).map(|page| page.range))
            .or_else(|| {
                self.page_index
                    .as_ref()
                    .and_then(|pages| pages.range(index))
            })
            .unwrap_or_default()
    }

    fn page_index_for_cursor(&self, cursor: DocumentCursor) -> Option<usize> {
        let page = self
            .pagination
            .as_ref()
            .and_then(|pages| pages.page_index_for_cursor(cursor))
            .or_else(|| {
                self.page_index
                    .as_ref()
                    .and_then(|pages| pages.page_index_for_cursor(cursor))
            });
        page.or_else(|| (self.page_count() == 1 && self.page_range(0).is_empty()).then_some(0))
    }

    fn page_index_after(&self, index: usize, next: bool) -> Option<usize> {
        let count = self.page_count();
        if next {
            index.checked_add(1).filter(|candidate| *candidate < count)
        } else {
            (index > 0 && index < count).then_some(index - 1)
        }
    }

    fn ensure_page(&mut self, index: usize, style: &ReaderStyle) -> Result<(), ReaderError> {
        if self.pagination.is_some() || self.page_cache.contains(index) {
            return Ok(());
        }
        let Some(page_index) = self.page_index.as_ref() else {
            return Ok(());
        };
        let page = Paginator::new(*style)
            .page_from_index(&self.layout, page_index, index)
            .ok_or(ReaderError::NoDocumentOpen)?;
        self.page_cache.insert(index, page);
        Ok(())
    }
}

#[derive(Default)]
struct PageCache {
    capacity: usize,
    entries: VecDeque<(usize, PageLayout)>,
}

impl PageCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
        }
    }

    fn get(&self, index: usize) -> Option<&PageLayout> {
        self.entries
            .iter()
            .find_map(|(cached_index, page)| (*cached_index == index).then_some(page))
    }

    fn contains(&self, index: usize) -> bool {
        self.entries
            .iter()
            .any(|(cached_index, _)| *cached_index == index)
    }

    fn insert(&mut self, index: usize, page: PageLayout) {
        if let Some(position) = self
            .entries
            .iter()
            .position(|(cached_index, _)| *cached_index == index)
        {
            self.entries.remove(position);
        }
        while self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back((index, page));
    }
}

struct DocumentCache {
    capacity: usize,
    entries: VecDeque<OpenDocument>,
}

impl DocumentCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::new(),
        }
    }

    fn take(&mut self, document: &DocumentId) -> Option<OpenDocument> {
        let position = self
            .entries
            .iter()
            .position(|entry| entry.location.document == *document)?;
        self.entries.remove(position)
    }

    fn insert(&mut self, document: OpenDocument) {
        if self.capacity == 0 {
            return;
        }
        self.entries
            .retain(|entry| entry.location.document != document.location.document);
        while self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(document);
    }
}

impl<P: ResourceProvider> Reader<P, ApproximateTextMeasurer, ComrakParser> {
    /// Create a reader using the built-in Comrak parser and deterministic
    /// approximate metrics. Applications with real fonts should use
    /// [`Reader::with_components`] and pass the same font metrics to layout.
    pub fn new(provider: P, style: ReaderStyle, viewport: Viewport) -> Self {
        Self::with_legacy_components(
            provider,
            ComrakParser::default(),
            ApproximateTextMeasurer,
            style,
            viewport,
        )
    }
}

impl<P, M, Parser> Reader<P, M, Parser>
where
    P: ResourceProvider,
    M: TextMeasurer + Clone,
    Parser: MarkdownParser,
{
    /// Create a reader with explicit parser and layout metric components.
    pub fn with_components(
        provider: P,
        parser: Parser,
        measurer: M,
        style: ReaderStyle,
        viewport: Viewport,
    ) -> Self {
        Self::with_limits(
            provider,
            parser,
            measurer,
            style,
            viewport,
            ReaderLimits::default(),
        )
    }

    /// Create a reader with explicit resource bounds. Page turns use the
    /// compact page directory and bounded page cache; they never reparse the
    /// current Markdown source.
    pub fn with_limits(
        provider: P,
        parser: Parser,
        measurer: M,
        style: ReaderStyle,
        viewport: Viewport,
        limits: ReaderLimits,
    ) -> Self {
        Self {
            provider,
            measurer,
            parser,
            style,
            viewport,
            current: None,
            document_cache: DocumentCache::new(limits.max_cached_documents),
            history: Vec::new(),
            history_index: 0,
            limits,
            eager_pagination: false,
        }
    }

    fn with_legacy_components(
        provider: P,
        parser: Parser,
        measurer: M,
        style: ReaderStyle,
        viewport: Viewport,
    ) -> Self {
        Self {
            provider,
            measurer,
            parser,
            style,
            viewport,
            current: None,
            document_cache: DocumentCache::new(0),
            history: Vec::new(),
            history_index: 0,
            limits: ReaderLimits {
                max_history_entries: usize::MAX,
                max_cached_documents: 0,
                page_cache_capacity: usize::MAX,
                image_retained_bytes: crate::image::DEFAULT_RETAINED_BYTES,
                image_entry_capacity: crate::image::DEFAULT_IMAGE_ENTRIES,
            },
            eager_pagination: true,
        }
    }

    /// Replace the parser while retaining the provider and layout settings.
    pub fn with_parser<OtherParser: MarkdownParser>(
        self,
        parser: OtherParser,
    ) -> Reader<P, M, OtherParser> {
        Reader {
            provider: self.provider,
            measurer: self.measurer,
            parser,
            style: self.style,
            viewport: self.viewport,
            current: self.current,
            document_cache: self.document_cache,
            history: self.history,
            history_index: self.history_index,
            limits: self.limits,
            eager_pagination: self.eager_pagination,
        }
    }

    /// Replace the layout measurer while retaining the parser and reader
    /// settings.
    pub fn with_measurer<OtherMeasurer: TextMeasurer + Clone>(
        self,
        measurer: OtherMeasurer,
    ) -> Reader<P, OtherMeasurer, Parser> {
        Reader {
            provider: self.provider,
            measurer,
            parser: self.parser,
            style: self.style,
            viewport: self.viewport,
            current: self.current,
            document_cache: self.document_cache,
            history: self.history,
            history_index: self.history_index,
            limits: self.limits,
            eager_pagination: self.eager_pagination,
        }
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }

    pub fn style(&self) -> ReaderStyle {
        self.style
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn limits(&self) -> ReaderLimits {
        self.limits
    }

    pub fn document(&self) -> Option<&Document> {
        self.current.as_ref().map(|current| &current.document)
    }

    pub fn layout(&self) -> Option<&DocumentLayout> {
        self.current.as_ref().map(|current| &current.layout)
    }

    /// Return eager display-list pagination when using the compatibility
    /// constructor. Bounded readers expose ranges through
    /// [`Self::pagination_index`] and retain display lists through
    /// [`Self::current_page`].
    pub fn pagination(&self) -> Option<&Pagination> {
        self.current
            .as_ref()
            .and_then(|current| current.pagination.as_ref())
    }

    pub fn current_location(&self) -> Option<&DocumentLocation> {
        self.current.as_ref().map(|current| &current.location)
    }

    /// The current page index, or `None` before the first document is opened.
    pub fn current_page_index(&self) -> Option<usize> {
        self.current.as_ref().map(|current| current.page)
    }

    /// The one-based page number stored on [`PageLayout`].
    pub fn current_page_number(&self) -> Option<usize> {
        self.current_page().map(|page| page.number)
    }

    pub fn page_count(&self) -> usize {
        self.current.as_ref().map_or(0, OpenDocument::page_count)
    }

    pub fn current_page(&self) -> Option<&PageLayout> {
        self.current.as_ref().and_then(|current| current.page())
    }

    /// Return the compact page directory in bounded mode.
    pub fn pagination_index(&self) -> Option<&PaginationIndex> {
        self.current
            .as_ref()
            .and_then(|current| current.page_index.as_ref())
    }

    pub fn cache_stats(&self) -> ReaderCacheStats {
        let (cached_pages, image_retained_bytes, image_entries) =
            self.current.as_ref().map_or((0, 0, 0), |current| {
                (
                    current.page_cache.entries.len(),
                    current.images.retained_bytes(),
                    current.images.entry_count(),
                )
            });
        ReaderCacheStats {
            history_entries: self.history.len(),
            history_limit: self.limits.max_history_entries,
            cached_documents: self.document_cache.entries.len(),
            document_cache_limit: self.limits.max_cached_documents,
            cached_pages,
            page_cache_limit: self.limits.page_cache_capacity.max(1),
            image_retained_bytes,
            image_retained_limit: self.limits.image_retained_bytes,
            image_entries,
            image_entry_limit: self.limits.image_entry_capacity,
        }
    }

    pub fn current_cursor(&self) -> Option<DocumentCursor> {
        self.current.as_ref().map(|current| current.cursor)
    }

    /// Return the current content anchor used by layout-independent history.
    pub fn current_content_anchor(&self) -> Option<ContentAnchor> {
        self.current
            .as_ref()
            .map(|current| content_anchor_for(&current.document, &current.layout, current.cursor))
    }

    pub fn history(&self) -> &[ReadingLocation] {
        &self.history
    }

    pub fn can_go_back(&self) -> bool {
        self.history_index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_index + 1 < self.history.len()
    }

    /// Rebuild the current document with a new viewport and style.
    ///
    /// The current content anchor and every history entry remain stable while
    /// the layout and page directory are rebuilt. Cached inactive documents
    /// are discarded because their layouts use the old settings; they are
    /// rebuilt from their retained semantic documents when history reaches
    /// them.
    pub fn reflow(
        &mut self,
        viewport: Viewport,
        style: ReaderStyle,
    ) -> Result<ReaderEvent, ReaderError> {
        let Some(current) = self.current.take() else {
            return Err(ReaderError::NoDocumentOpen);
        };
        let content = content_anchor_for(&current.document, &current.layout, current.cursor);
        let location = current.location.clone();
        self.viewport = viewport;
        self.style = style;
        self.document_cache.entries.clear();

        let rebuilt = self.build_open_document(&location, current.document.clone());
        let mut rebuilt = match rebuilt {
            Ok(rebuilt) => rebuilt,
            Err(error) => {
                self.current = Some(current);
                return Err(error);
            }
        };
        let cursor = cursor_for_content_anchor(&rebuilt.document, &rebuilt.layout, content)
            .unwrap_or(rebuilt.cursor);
        rebuilt.cursor = cursor;
        rebuilt.page = rebuilt
            .page_index_for_cursor(cursor)
            .unwrap_or(rebuilt.page.min(rebuilt.page_count().saturating_sub(1)));
        if let Err(error) = rebuilt.ensure_page(rebuilt.page, &self.style) {
            self.current = Some(current);
            return Err(error);
        }
        self.current = Some(rebuilt);
        self.update_current_history();
        Ok(self.page_changed_event())
    }

    /// Change only the viewport while preserving the current passage.
    pub fn set_viewport(&mut self, viewport: Viewport) -> Result<ReaderEvent, ReaderError> {
        self.reflow(viewport, self.style)
    }

    /// Change only the reader style while preserving the current passage.
    pub fn set_style(&mut self, style: ReaderStyle) -> Result<ReaderEvent, ReaderError> {
        self.reflow(self.viewport, style)
    }

    /// Open the provider's configured current document as a new session.
    pub fn open(&mut self) -> Result<ReaderEvent, ReaderError> {
        let path = self.provider.entry_point().to_owned();
        self.open_document(path)
    }

    /// Open a root-relative document as a new session and clear prior history.
    pub fn open_document(&mut self, path: impl AsRef<Path>) -> Result<ReaderEvent, ReaderError> {
        let path = path.as_ref().to_owned();
        let location = DocumentLocation::new(document_id(&path), None);
        let loaded = self.load_location(location)?;
        let event = ReaderEvent::Opened {
            location: loaded.location.clone(),
            page_count: loaded.page_count(),
        };
        let history = Self::snapshot_for(&loaded);
        self.current = Some(loaded);
        self.history = vec![history];
        self.history_index = 0;
        Ok(event)
    }

    /// Advance one page. `false` means the current page was already the last.
    pub fn next_page(&mut self) -> Result<bool, ReaderError> {
        let Some(current) = self.current.as_ref() else {
            return Err(ReaderError::NoDocumentOpen);
        };
        let Some(next) = current.page_index_after(current.page, true) else {
            return Ok(false);
        };
        self.ensure_current_page(next)?;
        let current = self.current.as_mut().expect("current page exists");
        current.page = next;
        current.cursor = current.page_range(next).start;
        self.update_current_history();
        Ok(true)
    }

    /// Move back one page. `false` means the current page was already first.
    pub fn previous_page(&mut self) -> Result<bool, ReaderError> {
        let Some(current) = self.current.as_ref() else {
            return Err(ReaderError::NoDocumentOpen);
        };
        let Some(previous) = current.page_index_after(current.page, false) else {
            return Ok(false);
        };
        self.ensure_current_page(previous)?;
        let current = self.current.as_mut().expect("current page exists");
        current.page = previous;
        current.cursor = current.page_range(previous).start;
        self.update_current_history();
        Ok(true)
    }

    /// Advance one page and return a device-independent change event.
    pub fn next_page_event(&mut self) -> Result<ReaderEvent, ReaderError> {
        if self.next_page()? {
            Ok(self.page_changed_event())
        } else {
            Ok(ReaderEvent::NoAction)
        }
    }

    /// Move back one page and return a device-independent change event.
    pub fn previous_page_event(&mut self) -> Result<ReaderEvent, ReaderError> {
        if self.previous_page()? {
            Ok(self.page_changed_event())
        } else {
            Ok(ReaderEvent::NoAction)
        }
    }

    /// Render the current page through a caller-owned renderer and target.
    pub fn render_current_page<T, E>(
        &mut self,
        renderer: &mut EmbeddedGraphicsRenderer<E>,
        target: &mut T,
    ) -> Result<(), ReaderRenderError<T::Error>>
    where
        T: DrawTarget,
        Rgb888: Into<T::Color>,
        E: crate::typography::TextEngine,
    {
        let page = self
            .current_page()
            .ok_or(ReaderRenderError::Reader(ReaderError::NoDocumentOpen))?;
        renderer
            .render(page, target)
            .map_err(ReaderRenderError::Target)
    }

    /// Hit-test a point in the current page's device-independent page space.
    pub fn hit_test(&self, point: Point) -> Result<Option<&HitRegion>, ReaderError> {
        let page = self.current_page().ok_or(ReaderError::NoDocumentOpen)?;
        Ok(page.hit_test(point))
    }

    /// Activate the semantic link at a point, returning external URLs to the
    /// caller without opening them.
    pub fn activate_at(&mut self, point: Point) -> Result<ReaderEvent, ReaderError> {
        let target = self.hit_test(point)?.map(|region| region.target.clone());
        let Some(target) = target else {
            return Ok(ReaderEvent::NoAction);
        };
        self.activate(target)
    }

    /// Activate an already resolved display-list target.
    pub fn activate(&mut self, target: NavigationTarget) -> Result<ReaderEvent, ReaderError> {
        match target {
            NavigationTarget::Location(location) => self.navigate_to_location(location),
            NavigationTarget::Anchor(anchor) => self.navigate_to_anchor(&anchor),
            NavigationTarget::Document(document) => self.follow_document(document.as_ref()),
            NavigationTarget::DocumentAnchor { document, anchor } => {
                self.follow_document_anchor(document.as_ref(), &anchor)
            }
            NavigationTarget::Asset(asset) => Ok(ReaderEvent::Asset(PathBuf::from(asset.as_ref()))),
            NavigationTarget::External(url) => Ok(ReaderEvent::ExternalUrl(url)),
        }
    }

    /// Navigate to an anchor in the currently open document.
    pub fn navigate_to_anchor(&mut self, anchor: &str) -> Result<ReaderEvent, ReaderError> {
        let Some(current) = self.current.as_ref() else {
            return Err(ReaderError::NoDocumentOpen);
        };
        let cursor = anchor_cursor(&current.layout, &current.location, anchor, |cursor| {
            current.page_index_for_cursor(cursor)
        })?;
        let mut location = current.location.clone();
        location.anchor = Some(anchor.to_owned());
        self.navigate_loaded(location, cursor, None)
    }

    /// Follow a Markdown reference relative to the current document.
    pub fn follow_document(&mut self, reference: &str) -> Result<ReaderEvent, ReaderError> {
        let target = self.resolve_reference(reference)?;
        match target {
            ResourceTarget::Document(path) => {
                self.navigate_to_location(DocumentLocation::new(document_id(&path), None))
            }
            ResourceTarget::DocumentAnchor { document, anchor } => self
                .navigate_to_location(DocumentLocation::new(document_id(&document), Some(anchor))),
            other => Err(ReaderError::UnsupportedTarget(other)),
        }
    }

    /// Follow a Markdown file and land on its anchor.
    pub fn follow_document_anchor(
        &mut self,
        reference: &str,
        anchor: &str,
    ) -> Result<ReaderEvent, ReaderError> {
        let reference = format!("{reference}#{anchor}");
        self.follow_document(&reference)
    }

    /// Resolve and follow any reader link, including an external URL event.
    pub fn follow_reference(&mut self, reference: &str) -> Result<ReaderEvent, ReaderError> {
        match self.resolve_reference(reference)? {
            ResourceTarget::External(url) => Ok(ReaderEvent::ExternalUrl(url)),
            ResourceTarget::Anchor(anchor) => self.navigate_to_anchor(&anchor),
            ResourceTarget::Document(path) => {
                self.navigate_to_location(DocumentLocation::new(document_id(&path), None))
            }
            ResourceTarget::DocumentAnchor { document, anchor } => self
                .navigate_to_location(DocumentLocation::new(document_id(&document), Some(anchor))),
            ResourceTarget::Asset(path) => Ok(ReaderEvent::Asset(path)),
        }
    }

    /// Restore the prior document and page/logical cursor.
    pub fn back(&mut self) -> Result<bool, ReaderError> {
        self.move_history(-1)
    }

    /// Re-apply a location after going back.
    pub fn forward(&mut self) -> Result<bool, ReaderError> {
        self.move_history(1)
    }

    pub fn back_event(&mut self) -> Result<ReaderEvent, ReaderError> {
        if self.back()? {
            Ok(self.history_event(false))
        } else {
            Ok(ReaderEvent::NoAction)
        }
    }

    pub fn forward_event(&mut self) -> Result<ReaderEvent, ReaderError> {
        if self.forward()? {
            Ok(self.history_event(true))
        } else {
            Ok(ReaderEvent::NoAction)
        }
    }

    fn resolve_reference(&self, reference: &str) -> Result<ResourceTarget, ReaderError> {
        let Some(current) = self.current.as_ref() else {
            return Err(ReaderError::NoDocumentOpen);
        };
        self.provider
            .resolve_reference_from(Path::new(current.location.document.as_ref()), reference)
            .map_err(ReaderError::Resource)
    }

    fn navigate_to_location(
        &mut self,
        location: DocumentLocation,
    ) -> Result<ReaderEvent, ReaderError> {
        let loaded = self.load_location(location)?;
        let cursor = loaded.cursor;
        self.navigate_loaded(loaded.location.clone(), cursor, Some(loaded))
    }

    fn navigate_loaded(
        &mut self,
        location: DocumentLocation,
        cursor: DocumentCursor,
        loaded: Option<OpenDocument>,
    ) -> Result<ReaderEvent, ReaderError> {
        if self
            .current
            .as_ref()
            .is_some_and(|current| current.location == location && current.cursor == cursor)
        {
            return Ok(ReaderEvent::NoAction);
        }
        let Some(mut next) = loaded else {
            let origin = self
                .current
                .as_ref()
                .map(Self::snapshot_for)
                .ok_or(ReaderError::NoDocumentOpen)?;
            let page = {
                let current = self.current.as_mut().ok_or(ReaderError::NoDocumentOpen)?;
                current.location = location.clone();
                current.cursor = cursor;
                current.page = current
                    .page_index_for_cursor(cursor)
                    .unwrap_or(current.page);
                current.page
            };
            self.ensure_current_page(page)?;
            let page_count = self.page_count();
            let destination = self
                .current
                .as_ref()
                .map(Self::snapshot_for)
                .ok_or(ReaderError::NoDocumentOpen)?;
            if let Some(entry) = self.history.get_mut(self.history_index) {
                *entry = origin;
            }
            self.push_history(destination);
            return Ok(ReaderEvent::Navigated {
                location,
                page,
                page_count,
            });
        };

        next.location = location;
        next.cursor = cursor;
        next.page = next.page_index_for_cursor(cursor).unwrap_or(next.page);
        next.ensure_page(next.page, &self.style)?;
        let history_entry = Self::snapshot_for(&next);
        self.update_current_history();
        self.push_history(history_entry);
        let event = ReaderEvent::Navigated {
            location: next.location.clone(),
            page: next.page,
            page_count: next.page_count(),
        };
        self.current = Some(next);
        Ok(event)
    }

    fn load_location(&mut self, location: DocumentLocation) -> Result<OpenDocument, ReaderError> {
        if let Some(mut cached) = self.document_cache.take(&location.document) {
            if let Err(error) = self.prepare_loaded(&mut cached, location) {
                self.document_cache.insert(cached);
                return Err(error);
            }
            self.cache_current();
            return Ok(cached);
        }
        let loaded = self.load_uncached(&location)?;
        self.cache_current();
        Ok(loaded)
    }

    fn load_uncached(&self, location: &DocumentLocation) -> Result<OpenDocument, ReaderError> {
        let path = PathBuf::from(location.document.as_ref());
        let source = self.provider.read_markdown(&path)?;
        let document = self.parser.parse(&source)?;
        self.build_open_document(location, document)
    }

    fn build_open_document(
        &self,
        location: &DocumentLocation,
        document: Document,
    ) -> Result<OpenDocument, ReaderError> {
        let path = PathBuf::from(location.document.as_ref());
        let image_width = self.viewport.width.saturating_sub(
            self.style
                .page_padding
                .left
                .saturating_add(self.style.page_padding.right),
        );
        let image_height = self.viewport.height.saturating_sub(
            self.style
                .page_padding
                .top
                .saturating_add(self.style.page_padding.bottom),
        );
        let images = ImageResources::from_document_with_limits(
            &self.provider,
            &path,
            &document,
            image_width,
            image_height,
            self.limits.image_retained_bytes,
            self.limits.image_entry_capacity,
        );
        let layout = LayoutEngine::with_measurer(self.style, self.measurer.clone())
            .layout_with_images(&document, self.viewport, &images);
        let paginator = Paginator::new(self.style);
        let (pagination, page_index) = if self.eager_pagination {
            (Some(paginator.paginate(&layout)), None)
        } else {
            (None, Some(paginator.index(&layout)))
        };
        let cursor = match location.anchor.as_deref() {
            Some(anchor) => anchor_cursor(&layout, location, anchor, |cursor| {
                pagination
                    .as_ref()
                    .and_then(|pages| pages.page_index_for_cursor(cursor))
                    .or_else(|| {
                        page_index
                            .as_ref()
                            .and_then(|pages| pages.page_index_for_cursor(cursor))
                    })
            })?,
            None => pagination
                .as_ref()
                .and_then(|pages| pages.page(0).map(|page| page.range.start))
                .or_else(|| {
                    page_index
                        .as_ref()
                        .and_then(|pages| pages.range(0).map(|range| range.start))
                })
                .unwrap_or(DocumentCursor::new(layout.blocks.len(), 0)),
        };
        let page = pagination
            .as_ref()
            .and_then(|pages| pages.page_index_for_cursor(cursor))
            .or_else(|| {
                page_index
                    .as_ref()
                    .and_then(|pages| pages.page_index_for_cursor(cursor))
            })
            .unwrap_or(0);
        let mut loaded = OpenDocument {
            location: location.clone(),
            document,
            layout,
            pagination,
            page_index,
            page_cache: PageCache::new(self.limits.page_cache_capacity),
            images,
            page,
            cursor,
        };
        loaded.ensure_page(page, &self.style)?;
        Ok(loaded)
    }

    fn prepare_loaded(
        &self,
        loaded: &mut OpenDocument,
        location: DocumentLocation,
    ) -> Result<(), ReaderError> {
        loaded.location = location.clone();
        if let Some(anchor) = location.anchor.as_deref() {
            loaded.cursor = anchor_cursor(&loaded.layout, &location, anchor, |cursor| {
                loaded.page_index_for_cursor(cursor)
            })?;
        } else {
            loaded.page = 0;
            loaded.cursor = loaded.page_range(0).start;
        }
        loaded.page = loaded
            .page_index_for_cursor(loaded.cursor)
            .unwrap_or(loaded.page);
        loaded.ensure_page(loaded.page, &self.style)?;
        Ok(())
    }

    fn cache_current(&mut self) {
        if let Some(current) = self.current.take() {
            self.document_cache.insert(current);
        }
    }

    fn ensure_current_page(&mut self, page: usize) -> Result<(), ReaderError> {
        let current = self.current.as_mut().ok_or(ReaderError::NoDocumentOpen)?;
        current.ensure_page(page, &self.style)
    }

    fn push_history(&mut self, entry: ReadingLocation) {
        self.history.truncate(self.history_index + 1);
        self.history.push(entry);
        let limit = self.limits.max_history_entries.max(1);
        if self.history.len() > limit {
            let remove = self.history.len() - limit;
            self.history.drain(..remove);
            self.history_index = self.history_index.saturating_sub(remove);
        } else {
            self.history_index = self.history.len() - 1;
        }
    }

    fn snapshot_for(current: &OpenDocument) -> ReadingLocation {
        ReadingLocation {
            location: current.location.clone(),
            page: current.page,
            cursor: current.cursor,
            content: content_anchor_for(&current.document, &current.layout, current.cursor),
        }
    }

    fn update_current_history(&mut self) {
        let Some(current) = self.current.as_ref() else {
            return;
        };
        if let Some(entry) = self.history.get_mut(self.history_index) {
            *entry = Self::snapshot_for(current);
        }
    }

    fn move_history(&mut self, direction: isize) -> Result<bool, ReaderError> {
        if self.current.is_none() {
            return Err(ReaderError::NoDocumentOpen);
        }
        self.update_current_history();
        let target_index = if direction < 0 {
            self.history_index.checked_sub(1)
        } else {
            (self.history_index + 1 < self.history.len()).then_some(self.history_index + 1)
        };
        let Some(target_index) = target_index else {
            return Ok(false);
        };
        let target = self.history[target_index].clone();
        let loaded = self.load_location(target.location.clone())?;
        let mut restored = loaded;
        let cursor =
            cursor_for_content_anchor(&restored.document, &restored.layout, target.content)
                .unwrap_or(target.cursor);
        restored.cursor = cursor;
        restored.page = restored
            .page_index_for_cursor(cursor)
            .unwrap_or(target.page.min(restored.page_count().saturating_sub(1)));
        restored.ensure_page(restored.page, &self.style)?;
        self.current = Some(restored);
        self.history_index = target_index;
        Ok(true)
    }

    fn page_changed_event(&self) -> ReaderEvent {
        ReaderEvent::PageChanged {
            page: self.current_page_index().unwrap_or(0),
            page_count: self.page_count(),
        }
    }

    fn history_event(&self, forward: bool) -> ReaderEvent {
        let location = self
            .current_location()
            .cloned()
            .expect("history movement requires an open document");
        let page = self.current_page_index().unwrap_or(0);
        let page_count = self.page_count();
        if forward {
            ReaderEvent::Forward {
                location,
                page,
                page_count,
            }
        } else {
            ReaderEvent::Back {
                location,
                page,
                page_count,
            }
        }
    }
}

fn document_id(path: &Path) -> DocumentId {
    DocumentId::from(path.to_string_lossy().replace('\\', "/"))
}

fn content_anchor_for(
    document: &Document,
    layout: &DocumentLayout,
    cursor: DocumentCursor,
) -> ContentAnchor {
    if cursor.block >= document.blocks().len() {
        return ContentAnchor::new(document.blocks().len(), 0);
    }
    let Some(block) = document.blocks().get(cursor.block) else {
        return ContentAnchor::new(document.blocks().len(), 0);
    };
    let Some(layout_block) = layout.blocks().get(cursor.block) else {
        return ContentAnchor::new(cursor.block, 0);
    };
    let starts = line_start_offsets(block, layout_block);
    let offset = starts
        .get(cursor.line)
        .copied()
        .unwrap_or_else(|| block.reading_text().chars().count());
    ContentAnchor::new(cursor.block, offset)
}

fn cursor_for_content_anchor(
    document: &Document,
    layout: &DocumentLayout,
    anchor: ContentAnchor,
) -> Option<DocumentCursor> {
    if anchor.block >= document.blocks().len() {
        return Some(DocumentCursor::new(document.blocks().len(), 0));
    }
    let block = document.blocks().get(anchor.block)?;
    let layout_block = layout.blocks().get(anchor.block)?;
    if layout_block.lines.is_empty() {
        return Some(DocumentCursor::new(anchor.block, 0));
    }
    let offset = anchor.offset.min(block.reading_text().chars().count());
    let starts = line_start_offsets(block, layout_block);
    let line = starts
        .iter()
        .enumerate()
        .rev()
        .find_map(|(line, start)| (*start <= offset).then_some(line))
        .unwrap_or(0);
    Some(DocumentCursor::new(anchor.block, line))
}

/// Derive source-content offsets from the rendered fragments in one block.
///
/// Layout fragments retain visible text but not source spans. Matching them
/// against the block's stable reading text is enough to ignore generated list
/// markers, table decorations, and page-only whitespace while preserving the
/// first visible content position of every line. Loaded images advance over
/// their visible fallback representation even though the rendered fragment has
/// no text glyphs.
fn line_start_offsets(
    block: &crate::document::Block,
    layout_block: &crate::layout::LayoutBlock,
) -> Vec<usize> {
    let source = block.reading_text().chars().collect::<Vec<_>>();
    let mut offset = 0;
    layout_block
        .lines
        .iter()
        .map(|line| {
            let mut line_start = skip_source_whitespace(&source, offset);
            let mut matched = false;
            for fragment in &line.fragments {
                if let Some(image) = &fragment.image {
                    let fallback = image_fallback(&image.alt);
                    if let Some(fragment_offset) = match_fragment(&source, offset, &fallback) {
                        if !matched {
                            line_start = fragment_offset;
                            matched = true;
                        }
                        offset = fragment_offset.saturating_add(fallback.chars().count());
                    } else if let Some(fragment_offset) =
                        match_fragment(&source, offset, &image.alt)
                    {
                        if !matched {
                            line_start = fragment_offset;
                            matched = true;
                        }
                        offset = fragment_offset.saturating_add(image.alt.chars().count());
                    }
                    continue;
                }
                if fragment.text.is_empty() {
                    continue;
                }
                let Some(fragment_offset) = match_fragment(&source, offset, &fragment.text) else {
                    continue;
                };
                if !matched {
                    line_start = fragment_offset;
                    matched = true;
                }
                offset = fragment_offset.saturating_add(fragment.text.chars().count());
            }
            line_start.min(source.len())
        })
        .collect()
}

fn skip_source_whitespace(source: &[char], offset: usize) -> usize {
    let mut offset = offset.min(source.len());
    while source
        .get(offset)
        .is_some_and(|character| character.is_whitespace())
    {
        offset += 1;
    }
    offset
}

fn match_fragment(source: &[char], offset: usize, fragment: &str) -> Option<usize> {
    let fragment = fragment.chars().collect::<Vec<_>>();
    if fragment.is_empty() {
        return Some(offset.min(source.len()));
    }
    let exact = |start: usize| {
        source
            .get(start..start.saturating_add(fragment.len()))
            .is_some_and(|candidate| candidate == fragment.as_slice())
    };
    if exact(offset) {
        return Some(offset);
    }
    if fragment[0].is_whitespace() {
        return None;
    }
    let skipped = skip_source_whitespace(source, offset);
    exact(skipped).then_some(skipped)
}

fn anchor_cursor(
    layout: &DocumentLayout,
    location: &DocumentLocation,
    anchor: &str,
    page_index_for_cursor: impl Fn(DocumentCursor) -> Option<usize>,
) -> Result<DocumentCursor, ReaderError> {
    let Some(block) = layout
        .blocks()
        .iter()
        .position(|block| block.anchor.as_deref() == Some(anchor))
    else {
        return Err(ReaderError::AnchorNotFound {
            document: PathBuf::from(location.document.as_ref()),
            anchor: anchor.to_owned(),
        });
    };
    let cursor = DocumentCursor::new(block, 0);
    if page_index_for_cursor(cursor).is_some() {
        Ok(cursor)
    } else {
        Err(ReaderError::AnchorNotFound {
            document: PathBuf::from(location.document.as_ref()),
            anchor: anchor.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Viewport;
    use crate::style::ReaderStyle;

    #[test]
    fn line_start_offsets_advance_past_unavailable_images() {
        let document = ComrakParser::default()
            .parse("Before the image. ![missing](missing.png) After the image.")
            .expect("parse document");
        let layout =
            LayoutEngine::new(ReaderStyle::default()).layout(&document, Viewport::new(48, 200));
        let starts = line_start_offsets(&document.blocks()[0], &layout.blocks()[0]);
        let image_end = document.blocks()[0]
            .reading_text()
            .find(" After the image.")
            .expect("image fallback in reading text");

        assert!(
            starts.iter().any(|start| *start >= image_end),
            "line starts {starts:?} did not advance beyond the unavailable image"
        );
        assert_eq!(
            document.blocks()[0].plain_text(),
            "Before the image. missing After the image."
        );
    }

    #[test]
    fn line_start_offsets_include_nested_list_children() {
        let document = ComrakParser::default()
            .parse("- item one\n  - nested child content that wraps onto another line")
            .expect("parse document");
        let layout =
            LayoutEngine::new(ReaderStyle::default()).layout(&document, Viewport::new(72, 200));
        let starts = line_start_offsets(&document.blocks()[0], &layout.blocks()[0]);
        let nested_start = "item one\n".chars().count();

        assert!(
            starts.iter().any(|start| *start >= nested_start),
            "line starts {starts:?} did not enter the nested child"
        );
        assert!(document.blocks()[0]
            .plain_text()
            .contains("nested child content"));
    }

    #[test]
    fn line_start_offsets_advance_through_table_cells_after_reflow() {
        let document = ComrakParser::default()
            .parse(
                "| one | this later cell contains enough words to wrap across several lines |
| --- | --- |
| three | the final cell also keeps its content after the table changes size |",
            )
            .expect("parse document");
        let style = ReaderStyle::default();
        let wide_layout = LayoutEngine::new(style).layout(&document, Viewport::new(72, 240));
        let wide_starts = line_start_offsets(&document.blocks()[0], &wide_layout.blocks()[0]);
        let reading_text = document.blocks()[0].reading_text();
        let later_cell_offset = reading_text
            .find("this later cell")
            .expect("later table cell in reading text");

        let wide_line = wide_starts
            .iter()
            .position(|start| *start >= later_cell_offset)
            .expect("wrapped later cell line");
        assert!(wide_line > 0, "later cell must not be the first table line");
        let anchor = content_anchor_for(&document, &wide_layout, DocumentCursor::new(0, wide_line));
        assert!(
            anchor.offset >= later_cell_offset,
            "anchor {anchor:?} did not enter the later table cell; starts were {wide_starts:?}"
        );

        let narrow_layout = LayoutEngine::new(style).layout(&document, Viewport::new(40, 240));
        let restored = cursor_for_content_anchor(&document, &narrow_layout, anchor)
            .expect("restore anchor after table reflow");
        let narrow_starts = line_start_offsets(&document.blocks()[0], &narrow_layout.blocks()[0]);
        assert!(
            narrow_starts[restored.line] >= later_cell_offset,
            "restored cursor {restored:?} left the later table cell; starts were {narrow_starts:?}"
        );
    }

    #[test]
    fn content_anchors_handle_empty_and_short_documents() {
        let empty = Document::new();
        let empty_layout =
            LayoutEngine::new(ReaderStyle::default()).layout(&empty, Viewport::new(48, 24));
        assert_eq!(
            content_anchor_for(&empty, &empty_layout, DocumentCursor::new(0, 0)),
            ContentAnchor::new(0, 0)
        );
        assert_eq!(
            cursor_for_content_anchor(&empty, &empty_layout, ContentAnchor::new(0, 0)),
            Some(DocumentCursor::new(0, 0))
        );

        let short = ComrakParser::default()
            .parse("# Short")
            .expect("parse short document");
        let short_layout =
            LayoutEngine::new(ReaderStyle::default()).layout(&short, Viewport::new(48, 24));
        let anchor = content_anchor_for(&short, &short_layout, DocumentCursor::new(0, 0));
        assert_eq!(anchor, ContentAnchor::new(0, 0));
        assert_eq!(
            cursor_for_content_anchor(&short, &short_layout, anchor),
            Some(DocumentCursor::new(0, 0))
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderSession {
    location: DocumentLocation,
    history: ReaderHistory,
    page: usize,
    page_count: usize,
}

impl ReaderSession {
    pub fn new(location: DocumentLocation, page_count: usize) -> Self {
        Self {
            history: ReaderHistory::new(location.clone()),
            location,
            page: 0,
            page_count: page_count.max(1),
        }
    }

    pub fn location(&self) -> &DocumentLocation {
        &self.location
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn page_count(&self) -> usize {
        self.page_count
    }

    pub fn set_page_count(&mut self, page_count: usize) {
        self.page_count = page_count.max(1);
        self.page = self.page.min(self.page_count - 1);
    }

    pub fn next_page(&mut self) -> bool {
        if self.page + 1 < self.page_count {
            self.page += 1;
            true
        } else {
            false
        }
    }

    pub fn previous_page(&mut self) -> bool {
        if self.page > 0 {
            self.page -= 1;
            true
        } else {
            false
        }
    }

    pub fn open(&mut self, target: NavigationTarget) -> bool {
        let location = match target {
            NavigationTarget::Location(location) => location,
            NavigationTarget::Anchor(anchor) => {
                DocumentLocation::new(self.location.document.clone(), Some(anchor))
            }
            NavigationTarget::Document(document) => DocumentLocation::new(document, None),
            NavigationTarget::DocumentAnchor { document, anchor } => {
                DocumentLocation::new(document, Some(anchor))
            }
            NavigationTarget::Asset(_) | NavigationTarget::External(_) => return false,
        };
        self.history.push(location.clone());
        self.location = location;
        self.page = 0;
        true
    }

    pub fn back(&mut self) -> bool {
        let Some(location) = self.history.back().cloned() else {
            return false;
        };
        if location == self.location {
            return false;
        }
        self.location = location;
        self.page = 0;
        true
    }

    pub fn forward(&mut self) -> bool {
        let Some(location) = self.history.forward().cloned() else {
            return false;
        };
        if location == self.location {
            return false;
        }
        self.location = location;
        self.page = 0;
        true
    }

    pub fn history(&self) -> &ReaderHistory {
        &self.history
    }
}
