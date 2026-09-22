use prs_markdown::document::Document;
use prs_markdown::parse::{ComrakParser, MarkdownParser, ParseError};
use prs_markdown::reader::ReaderEvent;
use prs_markdown::resources::{ResourceError, ResourceProvider, ResourceTarget};
use prs_markdown::{
    BrowserResourceProvider, ContentAnchor, DocumentId, DocumentLocation,
    FileSystemResourceProvider, NavigationTarget, Reader, ReaderLayout, ReaderLimits, ReaderStyle,
    ReadingProgress, Viewport, DEFAULT_FONT_SCALE_PERCENT, MAX_FONT_SCALE_PERCENT,
    MIN_FONT_SCALE_PERCENT,
    T1_LANDSCAPE_VIEWPORT, T1_VIEWPORT,
};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "prs-markdown-reader-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary resource root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct MemoryProvider {
    document_path: PathBuf,
    entry_point: PathBuf,
    text: HashMap<String, String>,
    binary: HashMap<String, Vec<u8>>,
}

impl ResourceProvider for MemoryProvider {
    fn document_path(&self) -> &Path {
        &self.document_path
    }

    fn entry_point(&self) -> &Path {
        &self.entry_point
    }

    fn read_text(&self, path: &Path) -> Result<String, ResourceError> {
        let key = path.to_string_lossy();
        self.text
            .get(key.as_ref())
            .cloned()
            .ok_or_else(|| ResourceError::new(format!("missing text resource: {key}")))
    }

    fn read_binary(&self, path: &Path) -> Result<Vec<u8>, ResourceError> {
        let key = path.to_string_lossy();
        self.binary
            .get(key.as_ref())
            .cloned()
            .ok_or_else(|| ResourceError::new(format!("missing binary resource: {key}")))
    }

    fn resolve_reference(&self, reference: &str) -> Result<ResourceTarget, ResourceError> {
        self.resolve_reference_from(&self.entry_point, reference)
    }

    fn resolve_reference_from(
        &self,
        containing_document: &Path,
        reference: &str,
    ) -> Result<ResourceTarget, ResourceError> {
        if reference.starts_with("https://") {
            return Ok(ResourceTarget::External(reference.to_owned()));
        }
        let (path, anchor) = reference
            .split_once('#')
            .map_or((reference, None), |(path, anchor)| (path, Some(anchor)));
        if path.is_empty() {
            return anchor.map_or_else(
                || Ok(ResourceTarget::Document(containing_document.to_owned())),
                |anchor| Ok(ResourceTarget::Anchor(anchor.to_owned())),
            );
        }
        let resolved = containing_document
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(path);
        let resolved = PathBuf::from(resolved.to_string_lossy().replace('\\', "/"));
        if resolved
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            Ok(match anchor {
                Some(anchor) => ResourceTarget::DocumentAnchor {
                    document: resolved,
                    anchor: anchor.to_owned(),
                },
                None => ResourceTarget::Document(resolved),
            })
        } else {
            Ok(ResourceTarget::Asset(resolved))
        }
    }
}

fn reader(root: &TestRoot) -> Reader<FileSystemResourceProvider> {
    let provider = FileSystemResourceProvider::new(root.path(), "index.md").expect("provider");
    let style = ReaderStyle {
        page_padding: prs_markdown::Insets::all(1),
        body: prs_markdown::TextStyle::new(10, 10),
        heading: prs_markdown::TextStyle::new(10, 10),
        code: prs_markdown::TextStyle::new(10, 10),
        paragraph_spacing: 0,
        heading_spacing_before: 0,
        heading_spacing_after: 0,
        ..ReaderStyle::default()
    };
    Reader::new(provider, style, Viewport::new(80, 32))
}

#[test]
fn reader_uses_a_non_filesystem_source_for_entry_documents_and_assets() {
    let provider = MemoryProvider {
        document_path: PathBuf::from("legacy.md"),
        entry_point: PathBuf::from("index.md"),
        text: HashMap::from([(String::from("index.md"), String::from("# In memory\n"))]),
        binary: HashMap::from([(String::from("cover.png"), vec![1, 2, 3])]),
    };
    let mut reader = Reader::new(provider, ReaderStyle::default(), Viewport::new(80, 32));

    assert!(matches!(reader.open(), Ok(ReaderEvent::Opened { .. })));
    assert_eq!(
        reader.current_location().unwrap().document.as_ref(),
        "index.md"
    );
    assert_eq!(
        reader.provider().read_binary(Path::new("cover.png")),
        Ok(vec![1, 2, 3])
    );
}

#[test]
fn browser_provider_loads_entry_point_and_follows_local_documents_and_assets() {
    let provider = BrowserResourceProvider::new(
        "README.md",
        [
            (
                PathBuf::from("README.md"),
                b"# Home\n\n[Chapter](docs/chapter.md)\n\n![Cover](assets/cover.png)\n".to_vec(),
            ),
            (PathBuf::from("docs/chapter.md"), b"# Chapter\n".to_vec()),
            (
                PathBuf::from("assets/cover.png"),
                include_bytes!("fixtures/assets/observatory.png").to_vec(),
            ),
        ],
    )
    .expect("create browser provider");
    let mut reader = Reader::new(provider, ReaderStyle::default(), Viewport::new(80, 32));

    assert!(matches!(reader.open(), Ok(ReaderEvent::Opened { .. })));
    assert_eq!(
        reader.current_location().unwrap().document.as_ref(),
        "README.md"
    );
    assert!(matches!(
        reader.follow_reference("docs/chapter.md"),
        Ok(ReaderEvent::Navigated { .. })
    ));
    assert_eq!(
        reader.current_location().unwrap().document.as_ref(),
        "docs/chapter.md"
    );
    assert_eq!(
        reader
            .provider()
            .read_binary(Path::new("assets/cover.png"))
            .unwrap(),
        include_bytes!("fixtures/assets/observatory.png").to_vec()
    );
    assert!(reader.follow_reference("../../../outside.md").is_err());
}

#[test]
fn reading_progress_is_scoped_to_the_active_document_and_last_page_is_complete() {
    let root = TestRoot::new();
    fs::write(root.path().join("index.md"), "line\n".repeat(500)).expect("write long entry");
    fs::write(root.path().join("short.md"), "# Short\n").expect("write short document");

    let mut reader = reader(&root);
    reader.open().expect("open long document");
    assert!(reader.page_count() > 1);
    assert_eq!(
        reader.reading_progress(),
        Some(ReadingProgress::new(0, reader.page_count()))
    );

    while reader.next_page().expect("advance long document") {}
    assert_eq!(
        reader.reading_progress(),
        Some(ReadingProgress::new(
            reader.current_page_index().expect("last page"),
            reader.page_count()
        ))
    );
    assert_eq!(
        reader
            .reading_progress()
            .expect("last-page progress")
            .filled_width(600),
        600
    );

    reader
        .open_document("short.md")
        .expect("open short document");
    assert_eq!(reader.reading_progress(), Some(ReadingProgress::new(0, 1)));
}

#[test]
fn reading_progress_recomputes_after_reflow_at_the_preserved_anchor() {
    let root = TestRoot::new();
    let source = (0..160)
        .map(|index| format!("## Section {index}\n\nStable text for reflow testing.\n\n"))
        .collect::<String>();
    fs::write(root.path().join("index.md"), source).expect("write reflow document");

    let mut reader = reader(&root);
    reader.open().expect("open document");
    assert!(reader.next_page().expect("advance to a later page"));
    let anchor = reader.current_content_anchor().expect("current anchor");
    let before = reader.reading_progress().expect("initial progress");

    reader
        .set_viewport(Viewport::new(44, 24))
        .expect("reflow document");

    let after = reader.reading_progress().expect("reflow progress");
    assert_eq!(reader.current_content_anchor(), Some(anchor));
    assert_eq!(after.current_page(), reader.current_page_index().unwrap());
    assert_eq!(after.page_count(), reader.page_count());
    assert_ne!(after, before);
}

#[derive(Clone)]
struct CountingParser {
    parses: Arc<AtomicUsize>,
}

impl MarkdownParser for CountingParser {
    fn parse(&self, source: &str) -> Result<Document, ParseError> {
        self.parses.fetch_add(1, Ordering::Relaxed);
        ComrakParser::new().parse(source)
    }
}

#[test]
fn opens_pages_follows_files_and_restores_the_link_origin() {
    let root = TestRoot::new();
    fs::write(
        root.path().join("index.md"),
        "# Home\n\nThis is a deliberately long introduction that places the links after the first page.\n\n[Chapter](chapter.md#details)\n\n[Website](https://example.com/read)\n",
    )
    .expect("write index");
    fs::write(
        root.path().join("chapter.md"),
        "# Chapter\n\nA chapter introduction.\n\nMore chapter content.\n\n## Details\n\nThe linked details section.\n",
    )
    .expect("write chapter");

    let mut reader = reader(&root);
    let opened = reader.open().expect("open index");
    assert!(matches!(opened, ReaderEvent::Opened { page_count, .. } if page_count > 1));

    let link_page = (0..reader.page_count())
        .find(|&page| {
            reader
                .pagination()
                .expect("pagination")
                .page(page)
                .expect("page")
                .hit_regions
                .iter()
                .any(|region| {
                    region.target
                        == NavigationTarget::DocumentAnchor {
                            document: DocumentId::from("chapter.md"),
                            anchor: "details".into(),
                        }
                })
        })
        .expect("chapter link page");
    while reader.current_page_index().expect("current page") < link_page {
        assert!(reader.next_page().expect("next page"));
    }
    let origin_page = reader.current_page_index().expect("origin page");
    let origin_cursor = reader.current_cursor().expect("origin cursor");
    let point = reader
        .current_page()
        .expect("current page")
        .hit_regions
        .iter()
        .find(|region| {
            region.target
                == NavigationTarget::DocumentAnchor {
                    document: DocumentId::from("chapter.md"),
                    anchor: "details".into(),
                }
        })
        .expect("chapter hit region")
        .bounds
        .top_left;

    let navigated = reader.activate_at(point).expect("follow chapter link");
    assert!(matches!(
        navigated,
        ReaderEvent::Navigated { ref location, .. }
            if location == &DocumentLocation::new(
                DocumentId::from("chapter.md"),
                Some("details".into())
            )
    ));
    assert!(reader.current_page_index().expect("chapter page") > 0);

    assert!(reader.back().expect("go back"));
    assert_eq!(
        reader.current_location(),
        Some(&DocumentLocation::new(DocumentId::from("index.md"), None,))
    );
    assert_eq!(reader.current_page_index(), Some(origin_page));
    assert_eq!(reader.current_cursor(), Some(origin_cursor));
    assert!(reader.forward().expect("go forward"));
    assert_eq!(
        reader.current_location().unwrap().anchor.as_deref(),
        Some("details")
    );
}

#[test]
fn reflow_preserves_the_current_passage_across_viewport_and_font_changes() {
    let root = TestRoot::new();
    let source = (0..24)
        .map(|index| {
            format!(
                "## Section {index}\n\nThe section contains stable words around the current reading position. This sentence is deliberately long so that pagination changes when the viewport and font size change.\n\n"
            )
        })
        .collect::<String>();
    fs::write(root.path().join("index.md"), source).expect("write document");

    let mut reader = reader(&root);
    reader.open().expect("open document");
    for _ in 0..3 {
        assert!(reader.next_page().expect("advance page"));
    }
    let original_anchor = reader
        .current_content_anchor()
        .expect("current content anchor");
    let original_style = reader.style();

    reader
        .set_viewport(Viewport::new(48, 24))
        .expect("narrow viewport");
    assert_eq!(
        reader.current_content_anchor(),
        Some(original_anchor),
        "viewport changes must keep the same passage"
    );

    let mut smaller_style = original_style;
    smaller_style.body.font_size = 7;
    smaller_style.body.line_height = 8;
    smaller_style.heading.font_size = 8;
    smaller_style.heading.line_height = 9;
    reader.set_style(smaller_style).expect("smaller font");
    assert_eq!(reader.current_content_anchor(), Some(original_anchor));

    reader
        .reflow(Viewport::new(80, 32), original_style)
        .expect("restore original layout");
    assert_eq!(reader.current_content_anchor(), Some(original_anchor));
}

#[test]
fn history_restores_content_anchors_after_reflow() {
    let root = TestRoot::new();
    fs::write(
        root.path().join("index.md"),
        "# Start\n\nA long opening passage with enough words to span several pages in the compact test viewport.\n\n## Finish\n\nThe destination passage.\n",
    )
    .expect("write document");

    let mut reader = reader(&root);
    reader.open().expect("open document");
    assert!(reader.next_page().expect("advance to origin"));
    let origin_anchor = reader.current_content_anchor().expect("origin anchor");
    reader
        .navigate_to_anchor("finish")
        .expect("navigate to finish");
    let finish_anchor = reader.current_content_anchor().expect("finish anchor");

    reader
        .set_viewport(Viewport::new(44, 24))
        .expect("reflow reader");
    assert_eq!(reader.current_content_anchor(), Some(finish_anchor));

    assert!(reader.back().expect("go back"));
    assert_eq!(reader.current_content_anchor(), Some(origin_anchor));
    assert!(reader.forward().expect("go forward"));
    assert_eq!(reader.current_content_anchor(), Some(finish_anchor));
}

#[test]
fn reflow_handles_empty_and_short_documents() {
    let root = TestRoot::new();
    fs::write(root.path().join("index.md"), "# Entry\n").expect("write entry");
    fs::write(root.path().join("empty.md"), "").expect("write empty document");
    fs::write(root.path().join("short.md"), "# Short\n").expect("write short document");

    let mut reader = reader(&root);
    reader
        .open_document("empty.md")
        .expect("open empty document");
    assert_eq!(
        reader.current_content_anchor(),
        Some(ContentAnchor::new(0, 0))
    );
    reader
        .set_viewport(Viewport::new(44, 24))
        .expect("reflow empty document");
    reader
        .set_style(reader.style())
        .expect("restyle empty document");
    assert!(!reader.next_page().expect("advance empty document"));

    reader
        .open_document("short.md")
        .expect("open short document");
    reader
        .reflow(Viewport::new(44, 24), reader.style())
        .expect("reflow short document");
    assert_eq!(
        reader.current_content_anchor(),
        Some(ContentAnchor::new(0, 0))
    );
    assert!(!reader.next_page().expect("advance short document"));
}

#[test]
fn font_size_controls_are_bounded_and_preserve_history_anchors() {
    let root = TestRoot::new();
    let source = (0..24)
        .map(|index| {
            format!(
                "## Section {index}\n\nThis passage keeps a stable reading position while the settings change the Markdown font size. It is long enough to wrap and repaginate in the compact test viewport.\n\n"
            )
        })
        .collect::<String>();
    fs::write(root.path().join("index.md"), source).expect("write document");

    let mut reader = reader(&root);
    reader.open().expect("open document");
    for _ in 0..3 {
        assert!(reader.next_page().expect("advance page"));
    }
    let origin_anchor = reader
        .current_content_anchor()
        .expect("origin content anchor");

    assert_eq!(reader.font_scale_percent(), DEFAULT_FONT_SCALE_PERCENT);
    assert_eq!(reader.style().body.font_size, 10);
    assert_eq!(reader.style().heading.font_size, 10);

    assert!(matches!(
        reader.decrease_font_size().expect("decrease font size"),
        ReaderEvent::PageChanged { .. }
    ));
    assert_eq!(reader.font_scale_percent(), MIN_FONT_SCALE_PERCENT);
    assert_eq!(reader.current_content_anchor(), Some(origin_anchor));
    assert_eq!(reader.style().body.font_size, 8);
    assert_eq!(reader.style().heading.font_size, 8);
    for _ in 0..4 {
        assert_eq!(
            reader.decrease_font_size().expect("hold minimum font size"),
            ReaderEvent::NoAction
        );
    }
    assert_eq!(reader.font_scale_percent(), MIN_FONT_SCALE_PERCENT);

    reader.reset_font_size().expect("reset font size");
    assert_eq!(reader.font_scale_percent(), DEFAULT_FONT_SCALE_PERCENT);
    assert_eq!(reader.current_content_anchor(), Some(origin_anchor));
    for _ in 0..4 {
        reader.increase_font_size().expect("increase font size");
    }
    assert_eq!(reader.font_scale_percent(), MAX_FONT_SCALE_PERCENT);
    assert_eq!(reader.current_content_anchor(), Some(origin_anchor));
    assert_eq!(reader.style().body.font_size, 15);
    assert_eq!(reader.style().heading.font_size, 15);
    for _ in 0..4 {
        assert_eq!(
            reader.increase_font_size().expect("hold maximum font size"),
            ReaderEvent::NoAction
        );
    }

    reader
        .navigate_to_anchor("section-12")
        .expect("navigate to history destination");
    let destination_anchor = reader
        .current_content_anchor()
        .expect("destination content anchor");
    reader
        .reset_font_size()
        .expect("reset destination font size");
    assert_eq!(reader.current_content_anchor(), Some(destination_anchor));
    assert!(reader.back().expect("restore origin through history"));
    assert_eq!(reader.current_content_anchor(), Some(origin_anchor));
    assert!(reader
        .forward()
        .expect("restore destination through history"));
    assert_eq!(reader.current_content_anchor(), Some(destination_anchor));
}

#[test]
fn font_size_extremes_keep_linked_image_hit_regions_usable() {
    let root = TestRoot::new();
    fs::create_dir_all(root.path().join("assets")).expect("create asset directory");
    fs::write(
        root.path().join("assets/observatory.png"),
        include_bytes!("fixtures/assets/observatory.png"),
    )
    .expect("write image fixture");
    fs::write(
        root.path().join("index.md"),
        "# Linked image\n\n[![Observatory](assets/observatory.png)](#finish)\n\n## Finish\n\nThe destination passage.\n",
    )
    .expect("write document");

    let mut reader = reader(&root);
    reader.open().expect("open document");
    for _ in 0..4 {
        reader.increase_font_size().expect("increase font size");
    }
    assert!(reader.next_page().expect("advance to linked image"));
    let max_page = reader.current_page().expect("maximum page");
    let image_link = max_page
        .hit_regions
        .iter()
        .find(|region| region.target == NavigationTarget::Anchor("finish".to_owned()))
        .expect("linked image hit region at maximum size");
    assert!(reader.viewport().contains(image_link.bounds.top_left));
    assert!(!image_link.bounds.is_zero_sized());
    assert!(reader
        .layout()
        .expect("maximum layout")
        .blocks()
        .iter()
        .flat_map(|block| block.lines.iter())
        .flat_map(|line| line.fragments.iter())
        .any(|fragment| fragment.image.is_some()));

    reader.reset_font_size().expect("reset font size");
    reader
        .decrease_font_size()
        .expect("decrease to minimum font size");
    let min_page = reader.current_page().expect("minimum page");
    let image_link = min_page
        .hit_regions
        .iter()
        .find(|region| region.target == NavigationTarget::Anchor("finish".to_owned()))
        .expect("linked image hit region at minimum size");
    assert!(reader.viewport().contains(image_link.bounds.top_left));
    assert!(!image_link.bounds.is_zero_sized());
}

#[test]
fn shared_layout_reflows_portrait_landscape_and_scaled_content_together() {
    let root = TestRoot::new();
    fs::create_dir_all(root.path().join("assets")).expect("create asset directory");
    fs::write(
        root.path().join("assets/observatory.png"),
        include_bytes!("fixtures/assets/observatory.png"),
    )
    .expect("write image fixture");
    fs::write(
        root.path().join("index.md"),
        "# Start\n\n[Finish](#finish)\n\n![Observatory](assets/observatory.png)\n\nA long passage with stable content that wraps differently when the reader changes orientation and font scale. This sentence repeats enough words to force pagination in both layouts.\n\n## Finish\n\nThe destination passage remains stable across presentation changes.\n",
    )
    .expect("write document");

    let provider = FileSystemResourceProvider::new(root.path(), "index.md").expect("provider");
    let base_style = ReaderStyle {
        page_padding: prs_markdown::Insets::all(4),
        body: prs_markdown::TextStyle::new(10, 10),
        heading: prs_markdown::TextStyle::new(12, 12),
        code: prs_markdown::TextStyle::new(10, 10),
        paragraph_spacing: 0,
        heading_spacing_before: 0,
        heading_spacing_after: 0,
        ..ReaderStyle::default()
    };
    let portrait = ReaderLayout::new(T1_VIEWPORT)
        .with_status_bar_height(76)
        .with_progress_line_height(16)
        .with_font_scale_percent(125);
    let mut reader = Reader::with_layout(
        provider,
        ComrakParser::default(),
        prs_markdown::ApproximateTextMeasurer,
        portrait,
        base_style,
    );
    reader.open().expect("open document");

    assert_eq!(reader.viewport(), Viewport::new(600, 708));
    assert_eq!(reader.style().body.font_size, 13);
    assert_eq!(reader.reader_layout(), portrait);
    let portrait_anchor = reader
        .current_content_anchor()
        .expect("initial content anchor");
    let portrait_image = reader
        .layout()
        .expect("portrait layout")
        .blocks()
        .iter()
        .flat_map(|block| block.lines.iter())
        .flat_map(|line| line.fragments.iter())
        .find_map(|fragment| {
            fragment
                .image
                .as_ref()
                .map(|image| (image.image.width(), image.image.height()))
        })
        .expect("portrait image command");
    let portrait_link = reader
        .current_page()
        .expect("portrait page")
        .hit_regions
        .first()
        .expect("portrait link")
        .bounds;

    let landscape = ReaderLayout::new(T1_LANDSCAPE_VIEWPORT)
        .with_status_bar_height(32)
        .with_progress_line_height(8)
        .with_font_scale_percent(80);
    reader
        .set_reader_layout(landscape)
        .expect("reflow to landscape");

    assert_eq!(reader.viewport(), Viewport::new(800, 560));
    assert_eq!(reader.style().body.font_size, 8);
    assert_eq!(reader.current_content_anchor(), Some(portrait_anchor));
    assert_eq!(reader.history().len(), 1);
    assert_eq!(
        reader.current_page().expect("landscape page").viewport(),
        reader.viewport()
    );
    assert!(reader
        .current_page()
        .expect("landscape page")
        .hit_regions
        .iter()
        .all(|region| reader.viewport().contains(region.bounds.top_left)));
    let landscape_page = reader.current_page().expect("landscape page");
    let landscape_image = reader
        .layout()
        .expect("landscape layout")
        .blocks()
        .iter()
        .flat_map(|block| block.lines.iter())
        .flat_map(|line| line.fragments.iter())
        .find_map(|fragment| {
            fragment
                .image
                .as_ref()
                .map(|image| (image.image.width(), image.image.height()))
        })
        .expect("landscape image command");
    assert_ne!(portrait_image, landscape_image);
    assert_ne!(portrait_link, landscape_page.hit_regions[0].bounds);

    reader
        .navigate_to_anchor("finish")
        .expect("navigate to destination");
    let finish_anchor = reader
        .current_content_anchor()
        .expect("destination content anchor");
    assert!(reader.back().expect("restore landscape start"));
    assert_eq!(reader.current_content_anchor(), Some(portrait_anchor));
    assert!(reader.forward().expect("restore landscape destination"));
    assert_eq!(reader.current_content_anchor(), Some(finish_anchor));
}

#[test]
fn anchors_and_external_links_are_actions_without_side_effects() {
    let root = TestRoot::new();
    fs::write(
        root.path().join("index.md"),
        "# Start\n\n[Local anchor](#finish)\n\n[External](https://example.com)\n\n## Finish\n",
    )
    .expect("write index");

    let mut reader = reader(&root);
    reader.open().expect("open index");
    let external = reader
        .follow_reference("https://example.com")
        .expect("external action");
    assert_eq!(
        external,
        ReaderEvent::ExternalUrl("https://example.com".into())
    );
    assert_eq!(reader.history().len(), 1);

    let navigated = reader
        .navigate_to_anchor("finish")
        .expect("navigate anchor");
    assert!(matches!(navigated, ReaderEvent::Navigated { .. }));
    assert_eq!(
        reader.current_location().unwrap().anchor.as_deref(),
        Some("finish")
    );
    assert!(reader.back().expect("restore start"));
    assert_eq!(reader.current_location().unwrap().anchor, None);
}

#[test]
fn bounded_reader_rebuilds_evicted_pages_without_reparsing() {
    let root = TestRoot::new();
    let source = (0..160)
        .map(|index| {
            format!(
                "## Section {index}\n\nLong prose line {index} with enough words to create multiple pages in the compact test viewport.\n\n"
            )
        })
        .collect::<String>();
    fs::write(root.path().join("index.md"), source).expect("write stress document");
    fs::write(
        root.path().join("chapter.md"),
        "# Chapter\n\nA linked chapter.\n",
    )
    .expect("write chapter");

    let parses = Arc::new(AtomicUsize::new(0));
    let provider = FileSystemResourceProvider::new(root.path(), "index.md").expect("provider");
    let style = ReaderStyle {
        page_padding: prs_markdown::Insets::all(1),
        body: prs_markdown::TextStyle::new(10, 10),
        heading: prs_markdown::TextStyle::new(10, 10),
        code: prs_markdown::TextStyle::new(10, 10),
        paragraph_spacing: 0,
        heading_spacing_before: 0,
        heading_spacing_after: 0,
        ..ReaderStyle::default()
    };
    let mut bounded = Reader::with_limits(
        provider,
        CountingParser {
            parses: parses.clone(),
        },
        prs_markdown::ApproximateTextMeasurer,
        style,
        Viewport::new(80, 32),
        ReaderLimits {
            max_history_entries: 2,
            max_cached_documents: 1,
            page_cache_capacity: 2,
            ..ReaderLimits::default()
        },
    );
    bounded.open().expect("open stress document");
    assert!(bounded.pagination().is_none());
    assert!(bounded.page_count() > 10);
    assert_eq!(parses.load(Ordering::Relaxed), 1);

    let mut eager = reader(&root);
    eager.open().expect("open eager reference");
    for _ in 0..8 {
        assert!(bounded.next_page().expect("next page"));
        assert!(eager.next_page().expect("next eager page"));
        assert!(bounded.current_page().is_some());
        assert_eq!(bounded.current_page(), eager.current_page());
        assert!(bounded.cache_stats().cached_pages <= 2);
    }
    let stats = bounded.cache_stats();
    assert!(stats.cached_pages <= stats.page_cache_limit);
    assert!(stats.history_entries <= stats.history_limit);

    bounded
        .follow_document("chapter.md")
        .expect("follow chapter");
    assert_eq!(parses.load(Ordering::Relaxed), 2);
    bounded.back().expect("return to stress document");
    bounded.forward().expect("return to chapter");
    assert_eq!(parses.load(Ordering::Relaxed), 2);
    assert!(bounded.cache_stats().cached_documents <= 1);
}
