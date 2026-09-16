use prs_markdown::{
    DocumentId, DocumentLocation, FileSystemResourceProvider, NavigationTarget, Reader,
    ReaderEvent, ReaderStyle, Viewport,
};
use std::fs;
use std::path::{Path, PathBuf};
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
