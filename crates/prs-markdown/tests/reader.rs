use prs_markdown::document::Document;
use prs_markdown::parse::{ComrakParser, MarkdownParser, ParseError};
use prs_markdown::reader::ReaderEvent;
use prs_markdown::resources::{ResourceError, ResourceProvider, ResourceTarget};
use prs_markdown::{
    DocumentId, DocumentLocation, FileSystemResourceProvider, NavigationTarget, Reader,
    ReaderLimits, ReaderStyle, Viewport,
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
