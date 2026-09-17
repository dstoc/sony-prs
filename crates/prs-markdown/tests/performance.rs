use prs_markdown::parse::ComrakParser;
use prs_markdown::{
    FileSystemResourceProvider, ImageResources, Insets, Paginator, Reader, ReaderStyle, Viewport,
};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const STRESS: &str = include_str!("fixtures/stress.md");
const LINKED: &str = include_str!("fixtures/stress-linked.md");
const IMAGE: &[u8] = include_bytes!("fixtures/assets/observatory.png");

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "prs-markdown-stress-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("assets")).expect("create stress root");
        fs::write(root.join("index.md"), STRESS.repeat(12)).expect("write stress document");
        fs::write(root.join("stress-linked.md"), LINKED).expect("write linked document");
        fs::write(root.join("assets/observatory.png"), IMAGE).expect("write stress image");
        Self(root)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn style() -> ReaderStyle {
    ReaderStyle {
        page_padding: Insets::all(2),
        paragraph_spacing: 3,
        heading_spacing_before: 4,
        heading_spacing_after: 2,
        ..ReaderStyle::default()
    }
}

fn rss_kib() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmRSS:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    })
}

#[test]
fn complete_stress_pipeline_reports_bounded_host_baseline() {
    let root = TempRoot::new();
    let provider = FileSystemResourceProvider::new(&root.0, "index.md").expect("provider");
    let source = provider
        .read_markdown(provider.document_path())
        .expect("read source");

    let parse_start = Instant::now();
    let document = ComrakParser::new()
        .parse(&source)
        .expect("parse stress corpus");
    let parse_time = parse_start.elapsed();

    let image_start = Instant::now();
    let images = ImageResources::from_document_with_limits(
        &provider,
        provider.document_path(),
        &document,
        596,
        760,
        4 * 1024 * 1024,
        128,
    );
    let image_time = image_start.elapsed();

    let layout_start = Instant::now();
    let layout = prs_markdown::LayoutEngine::new(style()).layout_with_images(
        &document,
        Viewport::new(600, 800),
        &images,
    );
    let layout_time = layout_start.elapsed();

    let pagination_start = Instant::now();
    let index = Paginator::new(style()).index(&layout);
    let pagination_time = pagination_start.elapsed();
    assert!(index.page_count() > 20);

    let mut reader = Reader::with_components(
        provider,
        ComrakParser::new(),
        prs_markdown::ApproximateTextMeasurer,
        style(),
        Viewport::new(600, 800),
    );
    let open_start = Instant::now();
    reader.open().expect("open stress corpus");
    let open_time = open_start.elapsed();
    assert!(reader.pagination().is_none());
    assert!(reader.current_page().is_some());
    assert!(reader.cache_stats().cached_pages <= 3);
    assert!(reader.cache_stats().image_retained_bytes <= 4 * 1024 * 1024);

    let mut next_time = Duration::ZERO;
    for _ in 0..10 {
        let turn_start = Instant::now();
        assert!(reader.next_page().expect("next stress page"));
        next_time += turn_start.elapsed();
    }
    let back_start = Instant::now();
    assert!(reader.previous_page().expect("previous stress page"));
    let previous_time = back_start.elapsed();
    assert!(reader.cache_stats().cached_pages <= reader.cache_stats().page_cache_limit);

    let rss = rss_kib().map_or_else(|| "unknown".to_owned(), |value| format!("{value} KiB"));
    eprintln!(
        "stress baseline: source={} KiB blocks={} pages={} parse={:?} images={:?} layout={:?} pagination={:?} open/first-page={:?} next10={:?} previous={:?} rss={}",
        source.len() / 1024,
        document.blocks().len(),
        reader.page_count(),
        parse_time,
        image_time,
        layout_time,
        pagination_time,
        open_time,
        next_time,
        previous_time,
        rss,
    );
}
