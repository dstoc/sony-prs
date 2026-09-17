//! Hardware-independent building blocks for a paginated Markdown reader.
//!
//! The crate deliberately stops at a generic
//! [`embedded_graphics::draw_target::DrawTarget`]
//! boundary.  A future parser, font backend, image decoder, and device UI can
//! be added without making the document engine know about a framebuffer or a
//! particular reader model. [`parse::ComrakParser`] converts CommonMark/GFM
//! input into the owned [`Document`] IR; Comrak nodes and arena lifetimes do
//! not appear in document, layout, pagination, or rendering types.
//! A caller supplies font bytes to [`typography::FontdueTextEngine`]; the
//! reader engine does not choose or bundle a licensed font family. A future
//! shaping backend, image decoder, and device UI can be added without making
//! the document engine know about a framebuffer or a particular reader model.

pub mod document;
pub mod geometry;
pub mod harness;
pub mod highlighting;
pub mod image;
pub mod layout;
pub mod navigation;
pub mod pagination;
pub mod parse;
pub mod reader;
pub mod render;
pub mod resources;
pub mod style;
pub mod typography;

pub use document::{
    AlertKind, Block, BlockMetadata, Document, Inline, ListItem, NodeId, SourcePosition,
    SourceSpan, Table, TableAlignment, TaskState,
};
pub use geometry::{intersection, translate, Rect, Viewport, T1_VIEWPORT};
pub use harness::{render_page, HostImage, HostReader};
pub use highlighting::{
    CodeHighlighter, HighlightSpan, HighlightedCode, HighlightedLine, SyntectHighlighter,
    SUPPORTED_LANGUAGES,
};
pub use image::{ImageResources, RasterImage, DEFAULT_RETAINED_BYTES, MAX_SOURCE_ALLOCATION};
pub use layout::{
    ApproximateTextMeasurer, DocumentLayout, LayoutBlock, LayoutBlockKind, LayoutEngine,
    LayoutFragment, LayoutImage, LayoutLine, TableColumnGroup, TableColumnLayout, TableLayout,
    TableLayoutMode, TableRowLayout, TextMeasurer,
};
pub use navigation::{DocumentId, DocumentLocation, NavigationTarget, ReaderHistory};
pub use pagination::{
    DisplayCommand, DisplayList, DocumentCursor, DocumentRange, HitRegion, LogicalPosition,
    LogicalRange, PageLayout, Pagination, Paginator,
};
pub use reader::{
    Reader, ReaderAction, ReaderError, ReaderEvent, ReaderRenderError, ReaderSession,
    ReadingLocation,
};
pub use render::EmbeddedGraphicsRenderer;
pub use resources::{
    FileSystemResourceProvider, FileSystemResources, ResourceProvider, ResourceTarget,
};
pub use style::{BorderStyle, Color, FillStyle, Insets, ReaderStyle, TextStyle};
pub use typography::{
    FontConfig, FontError, FontFace, FontLoadConfig, FontdueTextEngine, GlyphBitmap, LineMetrics,
    PositionedGlyph, SpanId, TextEngine, TextLayout, TextLine, TextMetrics, TextRun,
    TextStyle as TypographyStyle,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Block, Inline};
    use crate::layout::{LayoutEngine, Viewport};
    use crate::navigation::{DocumentId, DocumentLocation, ReaderHistory};
    use crate::pagination::Paginator;
    use crate::style::ReaderStyle;
    use embedded_graphics::geometry::Point;

    #[test]
    fn document_representation_owns_source_text() {
        let source = String::from("A paragraph with an owned link");
        let document =
            Document::from_blocks(vec![Block::Paragraph(vec![Inline::Text(source.clone())])]);
        drop(source);

        assert_eq!(
            document.blocks()[0].plain_text(),
            "A paragraph with an owned link"
        );
    }

    #[test]
    fn layout_and_pagination_use_the_host_viewport() {
        let document = Document::from_blocks(vec![Block::Paragraph(vec![Inline::Text(
            "one two three four five six seven eight nine ten eleven twelve".into(),
        )])]);
        let style = ReaderStyle {
            page_padding: style::Insets::all(2),
            body: style::TextStyle::new(10, 10),
            ..ReaderStyle::default()
        };
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(42, 20));
        let pages = Paginator::new(style).paginate(&layout);

        assert!(layout.blocks()[0].lines.len() > 1);
        assert!(pages.len() > 1);
        assert_eq!(pages[0].viewport(), Viewport::new(42, 20));
    }

    #[test]
    fn navigation_history_supports_back_and_forward() {
        let home = DocumentLocation::new(DocumentId::from("book.md"), None);
        let chapter = DocumentLocation::new(DocumentId::from("book.md"), Some("chapter".into()));
        let appendix = DocumentLocation::new(DocumentId::from("appendix.md"), None);
        let mut history = ReaderHistory::new(home.clone());

        history.push(chapter.clone());
        history.push(appendix.clone());
        assert_eq!(history.back(), Some(&chapter));
        assert_eq!(history.back(), Some(&home));
        assert_eq!(history.forward(), Some(&chapter));
        history.push(appendix.clone());
        assert_eq!(history.forward(), None);
        assert_eq!(history.current(), Some(&appendix));
    }

    #[test]
    fn pagination_retains_semantic_link_regions() {
        let document = Document::from_blocks(vec![Block::Paragraph(vec![Inline::Link {
            label: vec![Inline::Text("chapter".into())],
            destination: "book.md#chapter".into(),
            title: None,
        }])]);
        let style = ReaderStyle {
            page_padding: style::Insets::all(1),
            ..ReaderStyle::default()
        };
        let layout = LayoutEngine::new(style).layout(&document, Viewport::new(200, 100));
        let pages = Paginator::new(style).paginate(&layout);
        let hit = pages[0]
            .hit_test(Point::new(2, 2))
            .expect("link hit region");

        assert_eq!(
            hit.target,
            NavigationTarget::from_destination("book.md#chapter")
        );
    }
}
