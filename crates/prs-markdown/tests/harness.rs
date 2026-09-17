use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::Point;
use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::prelude::Pixel;
use prs_markdown::harness::{render_page, HostImage, HostReader};
use prs_markdown::layout::Viewport;
use prs_markdown::navigation::{DocumentId, NavigationTarget};
use prs_markdown::pagination::{DisplayCommand, DocumentCursor};
use prs_markdown::reader::ReaderSession;
use prs_markdown::style::{Insets, ReaderStyle, TextStyle};
use prs_markdown::typography::{
    FontConfig, FontFace, FontdueTextEngine, TextEngine, TextRun, TextStyle as FontTextStyle,
};
use prs_markdown::T1_VIEWPORT;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const CORPUS: &str = include_str!("fixtures/regression.md");
const BOUNDARY: &str = include_str!("fixtures/page-boundary.md");
const TASK_CONTROLS: &str = include_str!("fixtures/task-controls.md");
const ORDERED_LIST_START: &str = include_str!("fixtures/ordered-list-start.md");
const GOLDEN_REGULAR: &[u8] = notosans::REGULAR_TTF;
const GOLDEN_BOLD: &[u8] = notosans::BOLD_TTF;
const GOLDEN_ITALIC: &[u8] = notosans::ITALIC_TTF;
const GOLDEN_BOLD_ITALIC: &[u8] = notosans::BOLD_ITALIC_TTF;
// The notosans crate provides the four Noto Sans family faces, but no
// monospace face. Keep the reader's monospace slot font-backed and stable by
// using its regular face; code styling and face selection remain covered by
// the structural and Fontdue tests.
const GOLDEN_MONOSPACE: &[u8] = notosans::REGULAR_TTF;
const FIXTURES: &[(&str, &str)] = &[
    ("agent-output", include_str!("fixtures/agent-output.md")),
    ("agent-response", include_str!("fixtures/agent-response.md")),
    ("font-faces", include_str!("fixtures/font-faces.md")),
    ("linked-chapter", include_str!("fixtures/linked-chapter.md")),
    ("malformed", include_str!("fixtures/malformed.md")),
    ("modern-gfm", include_str!("fixtures/modern-gfm.md")),
    ("page-boundary", BOUNDARY),
    ("ordered-list-start", ORDERED_LIST_START),
    ("regression", CORPUS),
    (
        "syntax-highlight",
        include_str!("fixtures/syntax-highlight.md"),
    ),
    ("code-surface", include_str!("fixtures/code-surface.md")),
    (
        "tables-code-images",
        include_str!("fixtures/tables-code-images.md"),
    ),
    ("table-layout", include_str!("fixtures/table-layout.md")),
];

fn structural_style() -> ReaderStyle {
    ReaderStyle {
        page_padding: Insets::all(2),
        body: TextStyle::new(10, 12),
        heading: TextStyle {
            bold: true,
            ..TextStyle::new(16, 18)
        },
        code: TextStyle {
            code: true,
            ..TextStyle::new(10, 12)
        },
        paragraph_spacing: 4,
        heading_spacing_before: 5,
        heading_spacing_after: 3,
        list_indent: 12,
        block_quote_indent: 8,
        block_quote_padding: 2,
        ..ReaderStyle::default()
    }
}

fn narrow_list_style() -> ReaderStyle {
    ReaderStyle {
        page_padding: Insets::all(8),
        body: TextStyle::new(12, 14),
        heading: TextStyle {
            bold: true,
            ..TextStyle::new(14, 18)
        },
        paragraph_spacing: 0,
        heading_spacing_before: 0,
        heading_spacing_after: 0,
        list_indent: 18,
        list_item_spacing: 2,
        ..ReaderStyle::default()
    }
}

#[test]
fn corpus_exercises_semantics_boundaries_and_navigation() {
    let reader = HostReader::from_source(CORPUS, structural_style(), Viewport::new(96, 80))
        .expect("regression corpus should parse");

    assert!(reader.document().blocks().len() >= 10);
    assert!(reader.page_count() >= 3);
    assert!(reader
        .layout()
        .blocks()
        .iter()
        .flat_map(|block| block.lines.iter())
        .flat_map(|line| line.fragments.iter())
        .any(|fragment| fragment.style.bold));
    assert!(reader
        .layout()
        .blocks()
        .iter()
        .flat_map(|block| block.lines.iter())
        .flat_map(|line| line.fragments.iter())
        .any(|fragment| fragment.style.code));

    let visible = reader
        .pagination()
        .pages()
        .iter()
        .flat_map(|page| page.display_list().iter())
        .filter_map(|command| match command {
            DisplayCommand::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    assert!(visible.contains("Markdown"));
    assert!(visible.contains("regression"));
    assert!(visible.contains("corpus"));
    assert!(visible.contains("Completed"));
    assert!(visible.contains("task"));
    assert!(visible.contains("Table"));
    assert!(visible.contains("diagram"));

    let targets = reader
        .pagination()
        .pages()
        .iter()
        .flat_map(|page| page.hit_regions.iter().map(|region| &region.target))
        .collect::<Vec<_>>();
    assert!(
        targets.contains(&&NavigationTarget::Document(DocumentId::from(
            "linked-chapter.md"
        )))
    );
    assert!(targets.contains(&&NavigationTarget::DocumentAnchor {
        document: DocumentId::from("linked-chapter.md"),
        anchor: "installation".into(),
    }));
    assert!(targets.contains(&&NavigationTarget::Anchor("navigation".into())));

    let link_page = reader
        .pagination()
        .pages()
        .iter()
        .find(|page| !page.hit_regions.is_empty())
        .expect("corpus should produce link hit regions");
    let point = link_page.hit_regions[0].bounds.top_left;
    assert_eq!(
        reader.navigation_at(link_page.number - 1, point),
        Some(&link_page.hit_regions[0].target)
    );

    let mut session = ReaderSession::new(
        prs_markdown::navigation::DocumentLocation::new(DocumentId::from("regression.md"), None),
        reader.page_count(),
    );
    assert!(session.open(NavigationTarget::DocumentAnchor {
        document: DocumentId::from("linked-chapter.md"),
        anchor: "installation".into(),
    }));
    assert_eq!(session.location().document.as_ref(), "linked-chapter.md");
    assert_eq!(session.location().anchor.as_deref(), Some("installation"));
    assert!(session.back());
    assert_eq!(session.location().document.as_ref(), "regression.md");
}

#[test]
fn every_checked_in_fixture_runs_through_host_structure() {
    for (name, source) in FIXTURES {
        let reader = HostReader::from_source(source, structural_style(), Viewport::new(180, 120))
            .unwrap_or_else(|error| panic!("fixture {name} should parse: {error}"));
        assert!(
            !reader.document().blocks().is_empty(),
            "fixture {name} is empty"
        );
        assert!(reader.page_count() >= 1, "fixture {name} has no pages");
        assert!(
            reader
                .pagination()
                .pages()
                .iter()
                .flat_map(|page| page.display_list().iter())
                .any(|command| matches!(command, DisplayCommand::Text { .. })),
            "fixture {name} has no visible text"
        );
    }
}

#[test]
fn modern_gfm_fixture_has_alert_footnote_and_strike_output() {
    let source = include_str!("fixtures/modern-gfm.md");
    let reader = HostReader::from_source(source, structural_style(), Viewport::new(180, 120))
        .expect("modern GFM fixture should parse");

    assert!(reader
        .layout()
        .blocks()
        .iter()
        .any(|block| block.kind == prs_markdown::layout::LayoutBlockKind::Alert));
    assert!(reader
        .layout()
        .blocks()
        .iter()
        .any(|block| block.kind == prs_markdown::layout::LayoutBlockKind::Footnote));
    assert!(reader
        .layout()
        .blocks()
        .iter()
        .flat_map(|block| block.lines.iter())
        .flat_map(|line| line.fragments.iter())
        .any(|fragment| fragment.style.strikethrough));

    let visible = reader
        .pagination()
        .pages()
        .iter()
        .flat_map(|page| page.display_list().iter())
        .filter_map(|command| match command {
            DisplayCommand::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    assert!(visible.contains("Reader"));
    assert!(visible.contains("policy:"));
    assert!(visible.contains("[^reader]:"));
    assert!(visible.contains("flowchart"));
}

#[test]
fn syntax_highlighting_is_eink_styled_lossless_and_wraps_code() {
    let source = include_str!("fixtures/syntax-highlight.md");
    let reader = HostReader::from_source(source, structural_style(), Viewport::new(96, 80))
        .expect("syntax fixture should parse");

    let code_blocks = reader
        .document()
        .blocks()
        .iter()
        .filter_map(|block| match block {
            prs_markdown::Block::CodeBlock { language, code, .. } => {
                Some((language.as_deref(), code.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(code_blocks.len() >= 16);

    let code_layouts = reader
        .layout()
        .blocks()
        .iter()
        .filter(|block| block.kind == prs_markdown::layout::LayoutBlockKind::Code)
        .collect::<Vec<_>>();
    assert!(code_layouts
        .iter()
        .any(|block| block.lines.iter().any(|line| line.wrapped)));
    assert!(code_layouts
        .iter()
        .flat_map(|block| &block.lines)
        .flat_map(|line| &line.fragments)
        .any(|fragment| fragment.style.ink != 0));
    assert!(code_layouts
        .iter()
        .flat_map(|block| &block.lines)
        .flat_map(|line| &line.fragments)
        .any(|fragment| fragment.style.bold));
    assert!(code_layouts
        .iter()
        .flat_map(|block| &block.lines)
        .flat_map(|line| &line.fragments)
        .any(|fragment| fragment.style.italic));

    let unknown = code_layouts
        .iter()
        .zip(code_blocks.iter())
        .find(|(_, (language, _))| *language == Some("unknown-agent-language"))
        .expect("unknown code block should be present")
        .0;
    assert!(unknown
        .lines
        .iter()
        .flat_map(|line| &line.fragments)
        .all(|fragment| fragment.style.ink == 0 && !fragment.style.bold));

    for ((_, source_code), layout) in code_blocks.iter().zip(code_layouts.iter()) {
        let rendered = layout
            .lines
            .iter()
            .flat_map(|line| line.fragments.iter().map(|fragment| fragment.text.as_str()))
            .collect::<String>();
        let source_without_newlines = source_code.replace('\n', "");
        assert_eq!(rendered, source_without_newlines);
    }
}

#[test]
fn highlighted_code_renders_with_real_monospace_style_faces() {
    let regular = fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").expect("host font");
    let monospace =
        fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf").expect("monospace font");
    let monospace_bold = fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf")
        .expect("monospace bold font");
    let monospace_italic = fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Oblique.ttf")
        .expect("monospace italic font");
    let monospace_bold_italic =
        fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-BoldOblique.ttf")
            .expect("monospace bold-italic font");
    let font_engine = FontdueTextEngine::new(
        FontConfig::from_faces_with_monospace(
            &regular,
            &regular,
            &regular,
            &regular,
            &monospace,
            &monospace_bold,
            &monospace_italic,
            &monospace_bold_italic,
        ),
        128,
    )
    .expect("complete monospace family should parse");
    let source =
        "```rust\n// italic comment\nfn main() { let ready = true; }\n```\n\nThe combined code is ***`let ready`***.";
    let reader = HostReader::from_source_with_measurer(
        source,
        structural_style(),
        Viewport::new(240, 160),
        font_engine.clone(),
    )
    .expect("highlighted code should parse");
    let code_fragments = reader
        .layout()
        .blocks()
        .iter()
        .filter(|block| block.kind == prs_markdown::layout::LayoutBlockKind::Code)
        .flat_map(|block| &block.lines)
        .flat_map(|line| &line.fragments)
        .collect::<Vec<_>>();
    let all_code_fragments = reader
        .layout()
        .blocks()
        .iter()
        .flat_map(|block| &block.lines)
        .flat_map(|line| &line.fragments)
        .filter(|fragment| fragment.style.code)
        .collect::<Vec<_>>();
    assert!(code_fragments.iter().any(|fragment| fragment.style.bold));
    assert!(code_fragments.iter().any(|fragment| fragment.style.italic));
    assert!(all_code_fragments
        .iter()
        .any(|fragment| fragment.style.bold && fragment.style.italic));

    let mut renderer = prs_markdown::EmbeddedGraphicsRenderer::new(font_engine);
    for page in reader.pagination().pages() {
        let image = render_page(page, &mut renderer);
        assert!(image.pixels().iter().any(|pixel| *pixel < 255));
    }
}

#[test]
fn table_fixture_exposes_rows_headers_and_link_hit_regions() {
    let reader = HostReader::from_source(
        include_str!("fixtures/table-layout.md"),
        structural_style(),
        Viewport::new(96, 76),
    )
    .expect("table fixture should parse");
    let tables = reader
        .layout()
        .blocks()
        .iter()
        .filter_map(|block| block.table.as_ref())
        .collect::<Vec<_>>();

    assert!(tables.len() >= 2);
    assert!(tables
        .iter()
        .any(|table| table.rows.iter().any(|row| !row.header)));
    assert!(reader.page_count() > 1);
    assert!(reader
        .pagination()
        .pages()
        .iter()
        .flat_map(|page| page.hit_regions.iter())
        .any(|region| matches!(
            region.target,
            prs_markdown::NavigationTarget::External(ref url)
                if url == "https://example.invalid/sony-prs/14"
        )));
}

#[test]
fn exact_boundary_fixture_retains_canonical_page_ranges() {
    let style = ReaderStyle {
        page_padding: Insets::all(1),
        body: TextStyle::new(10, 20),
        heading: TextStyle {
            bold: true,
            ..TextStyle::new(10, 20)
        },
        paragraph_spacing: 0,
        heading_spacing_before: 0,
        heading_spacing_after: 0,
        ..ReaderStyle::default()
    };
    let reader = HostReader::from_source(BOUNDARY, style, Viewport::new(1000, 22))
        .expect("boundary fixture should parse");

    assert_eq!(reader.page_count(), 3);
    assert_eq!(
        reader.pagination().page(0).unwrap().logical_range(),
        prs_markdown::DocumentRange::new(DocumentCursor::new(0, 0), DocumentCursor::new(1, 0))
    );
    assert_eq!(
        reader.pagination().page(1).unwrap().logical_range(),
        prs_markdown::DocumentRange::new(DocumentCursor::new(1, 0), DocumentCursor::new(2, 0))
    );
    assert_eq!(
        reader.pagination().page(2).unwrap().logical_range(),
        prs_markdown::DocumentRange::new(DocumentCursor::new(2, 0), DocumentCursor::new(3, 0))
    );
    assert!(reader
        .visible_fragments(0)
        .unwrap()
        .join(" ")
        .contains("Exact"));
    assert!(reader
        .visible_fragments(1)
        .unwrap()
        .join(" ")
        .contains("placed"));
    assert!(reader
        .visible_fragments(2)
        .unwrap()
        .join(" ")
        .contains("following"));
}

#[test]
fn ordered_list_start_fixture_preserves_markers_across_pages() {
    let reader = HostReader::from_source(ORDERED_LIST_START, ReaderStyle::default(), T1_VIEWPORT)
        .expect("ordered-list fixture should parse");
    let visible = reader
        .pagination()
        .pages()
        .iter()
        .flat_map(|page| page.display_list().iter())
        .filter_map(|command| match command {
            DisplayCommand::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    assert!(reader.page_count() > 1);
    assert!(visible.contains("5. Establish"));
    assert!(visible.contains("8. Nested"));
    assert!(visible.contains("9. Nested"));
    assert!(visible.contains("18. Continue"));
}

#[test]
fn list_layout_matches_narrow_png_goldens() {
    let cases = [
        (
            "list-wrapped-unordered",
            include_str!("fixtures/list-wrapped-unordered.md"),
            Viewport::new(180, 120),
        ),
        (
            "list-wrapped-ordered",
            include_str!("fixtures/list-wrapped-ordered.md"),
            Viewport::new(180, 120),
        ),
        (
            "list-ordered-boundary",
            include_str!("fixtures/list-ordered-boundary.md"),
            Viewport::new(180, 320),
        ),
        (
            "list-page-continuation",
            include_str!("fixtures/list-page-continuation.md"),
            Viewport::new(180, 64),
        ),
    ];

    let font_engine = golden_font_engine();
    for (name, source, viewport) in cases {
        let reader = HostReader::from_source_with_measurer(
            source,
            narrow_list_style(),
            viewport,
            font_engine.clone(),
        )
        .unwrap_or_else(|error| panic!("fixture {name} should parse: {error}"));
        assert!(
            reader
                .pagination()
                .pages()
                .iter()
                .flat_map(|page| page.display_list())
                .any(|command| matches!(command, DisplayCommand::Text { .. })),
            "fixture {name} should render text"
        );
        if name == "list-page-continuation" {
            assert!(reader.page_count() > 1);
        }

        let mut renderer = prs_markdown::EmbeddedGraphicsRenderer::new(font_engine.clone());
        for (page_index, page) in reader.pagination().pages().iter().enumerate() {
            let image = render_page(page, &mut renderer);
            assert_png_golden(name, page_index + 1, &image);
        }
        remove_obsolete_fixture_goldens(name, reader.page_count());
    }
}

#[test]
fn every_checked_in_fixture_matches_png_goldens() {
    for (name, source) in FIXTURES {
        assert_fixture_goldens(name, source);
    }
}

#[test]
fn task_controls_match_the_narrow_viewport_golden() {
    let viewport = Viewport::new(160, 120);
    let font_engine = golden_font_engine();
    let reader = HostReader::from_source_with_measurer(
        TASK_CONTROLS,
        structural_style(),
        viewport,
        font_engine.clone(),
    )
    .expect("task controls fixture should parse");
    assert_eq!(reader.page_count(), 1);
    let mut renderer = prs_markdown::EmbeddedGraphicsRenderer::new(font_engine);
    let image = render_page(reader.page(0).expect("task controls page"), &mut renderer);
    assert_eq!(image.width(), viewport.width);
    assert_eq!(image.height(), viewport.height);
    assert_png_golden_at(
        golden_path("task-controls-narrow", 1),
        golden_failure_path("task-controls-narrow", 1),
        "task controls narrow viewport",
        &image,
    );
}

fn assert_fixture_goldens(name: &str, source: &str) {
    let viewport = T1_VIEWPORT;
    let font_engine = golden_font_engine();
    let reader = HostReader::from_source_with_measurer(
        source,
        ReaderStyle::default(),
        viewport,
        font_engine.clone(),
    )
    .unwrap_or_else(|error| panic!("fixture {name} should parse: {error}"));
    let mut renderer = prs_markdown::EmbeddedGraphicsRenderer::new(font_engine);

    for (page_index, page) in reader.pagination().pages().iter().enumerate() {
        let image = render_page(page, &mut renderer);
        assert_eq!(image.width(), T1_VIEWPORT.width);
        assert_eq!(image.height(), T1_VIEWPORT.height);
        assert_png_golden(name, page_index + 1, &image);
    }
    remove_obsolete_fixture_goldens(name, reader.page_count());
}

fn remove_obsolete_fixture_goldens(name: &str, page_count: usize) {
    if env::var_os("PRS_MARKDOWN_UPDATE_GOLDENS").is_none() {
        return;
    }

    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens");
    let prefix = format!("{name}-page-");
    let entries = fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("read golden directory {}: {error}", directory.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| panic!("read golden directory entry: {error}"));
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|file| file.to_str()) else {
            continue;
        };
        let Some(page) = file_name
            .strip_prefix(&prefix)
            .and_then(|page| page.strip_suffix(".png"))
            .and_then(|page| page.parse::<usize>().ok())
        else {
            continue;
        };
        if page > page_count {
            fs::remove_file(&path).unwrap_or_else(|error| {
                panic!("remove obsolete golden {}: {error}", path.display())
            });
        }
    }
}

fn golden_font_engine() -> FontdueTextEngine {
    FontdueTextEngine::new(
        FontConfig::from_faces(
            GOLDEN_REGULAR,
            GOLDEN_BOLD,
            GOLDEN_ITALIC,
            GOLDEN_BOLD_ITALIC,
            GOLDEN_MONOSPACE,
        ),
        512,
    )
    .expect("checked-in golden fonts should parse")
}

fn assert_png_golden(name: &str, page_number: usize, image: &HostImage) {
    assert_png_golden_at(
        golden_path(name, page_number),
        golden_failure_path(name, page_number),
        &format!("{name} page {page_number}"),
        image,
    );
}

fn assert_png_golden_at(golden: PathBuf, failure: PathBuf, label: &str, image: &HostImage) {
    let actual = image.png_bytes();

    if env::var_os("PRS_MARKDOWN_UPDATE_GOLDENS").is_some() {
        fs::write(&golden, &actual)
            .unwrap_or_else(|error| panic!("write golden {}: {error}", golden.display()));
        return;
    }

    let expected = match fs::read(&golden) {
        Ok(expected) => expected,
        Err(error) => {
            write_golden_failure(&failure, &actual);
            panic!(
                concat!(
                    "missing golden {} ({}); rendered output was written to {}. ",
                    "Inspect it, then promote inspected renders with `PRS_MARKDOWN_UPDATE_GOLDENS=1 ",
                    "cargo test -p prs-markdown --test harness`"
                ),
                golden.display(),
                error,
                failure.display()
            );
        }
    };

    if expected != actual {
        write_golden_failure(&failure, &actual);
        panic!(
            concat!(
                "PNG golden mismatch for {}; rendered output was written to {}. ",
                "Inspect it, then promote inspected renders with `PRS_MARKDOWN_UPDATE_GOLDENS=1 ",
                "cargo test -p prs-markdown --test harness`"
            ),
            label,
            failure.display()
        );
    }
}

fn golden_path(name: &str, page_number: usize) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(format!("{name}-page-{page_number:03}.png"))
}

fn golden_failure_path(name: &str, page_number: usize) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/prs-markdown-golden-failures")
        .join(format!("{name}-page-{page_number:03}.png"))
}

fn write_golden_failure(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("create golden failure directory: {error}"));
    }
    fs::write(path, bytes)
        .unwrap_or_else(|error| panic!("write golden failure {}: {error}", path.display()));
}

#[test]
fn host_image_is_a_grayscale_pgm_and_png_draw_target() {
    let mut image = HostImage::new(3, 2);
    image
        .draw_iter([Pixel(Point::new(1, 0), Rgb888::BLACK)])
        .unwrap();

    assert_eq!(image.pixel(Point::new(0, 0)), Some(255));
    assert_eq!(image.pixel(Point::new(1, 0)), Some(0));
    assert_eq!(image.pixel(Point::new(4, 0)), None);
    assert_eq!(
        image.pgm_bytes(),
        b"P5\n3 2\n255\n\xFF\0\xFF\xFF\xFF\xFF".to_vec()
    );
    let png = image.png_bytes();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert!(png.windows(4).any(|chunk| chunk == b"IHDR"));
    assert!(png.windows(4).any(|chunk| chunk == b"IEND"));
}

#[test]
fn host_render_helper_uses_the_production_renderer() {
    let reader = HostReader::from_source(
        "# Host page\n\nA visible paragraph.",
        structural_style(),
        Viewport::new(120, 80),
    )
    .unwrap();
    let mut renderer = prs_markdown::EmbeddedGraphicsRenderer::new(golden_font_engine());
    let image = render_page(reader.page(0).unwrap(), &mut renderer);

    assert_eq!(image.width(), 120);
    assert_eq!(image.height(), 80);
    assert!(image.pixels().iter().any(|pixel| *pixel < 255));
}

// Keep one standalone renderer smoke golden small; the fixture corpus above is
// the device-sized visual regression suite.
#[test]
fn rendered_host_page_matches_checked_in_png_golden() {
    let font_engine = golden_font_engine();
    let reader = HostReader::from_source_with_measurer(
        "# Host page\n\nA visible paragraph.",
        structural_style(),
        Viewport::new(120, 80),
        font_engine.clone(),
    )
    .unwrap();
    let mut renderer = prs_markdown::EmbeddedGraphicsRenderer::new(font_engine);
    let image = render_page(reader.page(0).unwrap(), &mut renderer);
    assert!(image.pixels().iter().any(|pixel| *pixel < 255));

    assert_png_golden_at(
        host_page_golden_path(),
        host_page_golden_failure_path(),
        "host page",
        &image,
    );
}

fn host_page_golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/host-page.png")
}

fn host_page_golden_failure_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/prs-markdown-golden-failures/host-page.png")
}

#[test]
fn golden_font_assets_cover_each_reader_face() {
    let mut engine = golden_font_engine();
    let family_faces = [
        FontFace::Regular,
        FontFace::Bold,
        FontFace::Italic,
        FontFace::BoldItalic,
    ];
    let rasters = family_faces
        .into_iter()
        .map(|face| {
            let layout = engine.layout(&[TextRun::new("Ag", FontTextStyle::new(face, 20))], 100);
            let glyph = layout
                .glyphs()
                .first()
                .expect("font should produce a glyph");
            assert!(
                glyph.width > 0,
                "{face:?} glyph should have visible geometry"
            );
            TextEngine::rasterize_glyph(&mut engine, glyph).alpha
        })
        .collect::<Vec<_>>();

    assert!(rasters
        .iter()
        .all(|alpha| alpha.iter().any(|pixel| *pixel > 0)));
    for (index, raster) in rasters.iter().enumerate() {
        assert!(
            rasters[..index].iter().all(|previous| previous != raster),
            "golden face {index} should have distinct glyph coverage"
        );
    }

    let monospace = engine.layout(&[TextRun::new("Ag", FontTextStyle::monospace(20))], 100);
    let glyph = monospace
        .glyphs()
        .first()
        .expect("monospace slot should produce a glyph");
    assert!(
        glyph.width > 0,
        "monospace glyph should have visible geometry"
    );
    assert!(TextEngine::rasterize_glyph(&mut engine, glyph)
        .alpha
        .iter()
        .any(|pixel| *pixel > 0));
}
