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
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const CORPUS: &str = include_str!("fixtures/regression.md");
const BOUNDARY: &str = include_str!("fixtures/page-boundary.md");
const HOST_PAGE_GOLDEN: &[u8] = include_bytes!("goldens/host-page.png");
const FIXTURES: &[(&str, &str)] = &[
    ("agent-output", include_str!("fixtures/agent-output.md")),
    ("agent-response", include_str!("fixtures/agent-response.md")),
    ("linked-chapter", include_str!("fixtures/linked-chapter.md")),
    ("malformed", include_str!("fixtures/malformed.md")),
    ("page-boundary", BOUNDARY),
    ("regression", CORPUS),
    (
        "tables-code-images",
        include_str!("fixtures/tables-code-images.md"),
    ),
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
fn every_checked_in_fixture_matches_png_goldens() {
    for (name, source) in FIXTURES {
        assert_fixture_goldens(name, source);
    }
}

fn assert_fixture_goldens(name: &str, source: &str) {
    let viewport = Viewport::new(180, 120);
    let reader = HostReader::from_source(source, structural_style(), viewport)
        .unwrap_or_else(|error| panic!("fixture {name} should parse: {error}"));
    let mut renderer = prs_markdown::EmbeddedGraphicsRenderer::new(TestTextEngine);

    for (page_index, page) in reader.pagination().pages().iter().enumerate() {
        let image = render_page(page, &mut renderer);
        assert_png_golden(name, page_index + 1, &image);
    }
}

fn assert_png_golden(name: &str, page_number: usize, image: &HostImage) {
    let golden = golden_path(name, page_number);
    let actual = image.png_bytes();

    if env::var_os("PRS_MARKDOWN_UPDATE_GOLDENS").is_some() {
        fs::write(&golden, &actual)
            .unwrap_or_else(|error| panic!("write golden {}: {error}", golden.display()));
        return;
    }

    let failure = golden_failure_path(name, page_number);
    let expected = match fs::read(&golden) {
        Ok(expected) => expected,
        Err(error) => {
            write_golden_failure(&failure, &actual);
            panic!(
                concat!(
                    "missing golden {} ({}); rendered output was written to {}. ",
                    "Inspect it, then promote it with `PRS_MARKDOWN_UPDATE_GOLDENS=1 ",
                    "cargo test -p prs-markdown --test harness every_checked_in_fixture_matches_png_goldens`"
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
                "PNG golden mismatch for {} page {}; rendered output was written to {}. ",
                "Inspect it, then promote it with `PRS_MARKDOWN_UPDATE_GOLDENS=1 ",
                "cargo test -p prs-markdown --test harness every_checked_in_fixture_matches_png_goldens`"
            ),
            name,
            page_number,
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
    let mut renderer = prs_markdown::EmbeddedGraphicsRenderer::new(TestTextEngine);
    let image = render_page(reader.page(0).unwrap(), &mut renderer);

    assert_eq!(image.width(), 120);
    assert_eq!(image.height(), 80);
    assert!(image.pixels().iter().any(|pixel| *pixel < 255));
}

#[test]
fn rendered_host_page_matches_checked_in_png_golden() {
    let reader = HostReader::from_source(
        "# Host page\n\nA visible paragraph.",
        structural_style(),
        Viewport::new(120, 80),
    )
    .unwrap();
    let mut renderer = prs_markdown::EmbeddedGraphicsRenderer::new(TestTextEngine);
    let image = render_page(reader.page(0).unwrap(), &mut renderer);
    assert!(image.pixels().iter().any(|pixel| *pixel < 255));
    assert_eq!(image.png_bytes(), HOST_PAGE_GOLDEN);
}

#[derive(Debug)]
struct TestTextEngine;

impl prs_markdown::TextEngine for TestTextEngine {
    fn measure(&self, _run: &prs_markdown::TextRun<'_>) -> prs_markdown::TextMetrics {
        prs_markdown::TextMetrics {
            advance_width: 6.0,
            width: 6,
            line: prs_markdown::LineMetrics {
                ascent: 7,
                descent: 1,
                line_gap: 0,
                line_height: 10,
                baseline: 7,
            },
        }
    }

    fn wrap(
        &self,
        runs: &[prs_markdown::TextRun<'_>],
        _available_width: u32,
    ) -> prs_markdown::TextLayout {
        let text = runs.iter().map(|run| run.text).collect::<String>();
        let glyphs = text
            .chars()
            .enumerate()
            .map(|(index, character)| prs_markdown::PositionedGlyph {
                character,
                glyph_id: 0,
                face: prs_markdown::FontFace::Regular,
                font_size: 10,
                x: index as i32 * 6,
                y: 0,
                width: 5,
                height: 7,
                advance_width: 6.0,
                byte_offset: index,
                span_id: None,
            })
            .collect::<Vec<_>>();
        prs_markdown::TextLayout {
            lines: vec![prs_markdown::TextLine {
                glyph_range: 0..glyphs.len(),
                width: glyphs.len() as u32 * 6,
                metrics: prs_markdown::LineMetrics {
                    ascent: 7,
                    descent: 1,
                    line_gap: 0,
                    line_height: 10,
                    baseline: 7,
                },
            }],
            glyphs,
        }
    }

    fn rasterize_glyph(
        &mut self,
        glyph: &prs_markdown::PositionedGlyph,
    ) -> prs_markdown::GlyphBitmap {
        let width = 5;
        let height = 7;
        let alpha = (0..height)
            .flat_map(|row| {
                (0..width).map(move |column| {
                    let edge = row == 0 || row == height - 1 || column == 0 || column == width - 1;
                    let mark = (u32::from(glyph.character) + row + column) % 7 == 0;
                    if !glyph.character.is_whitespace() && (edge || mark) {
                        255
                    } else {
                        0
                    }
                })
            })
            .collect();
        prs_markdown::GlyphBitmap {
            width,
            height,
            left: 0,
            top: 0,
            advance_width: 6.0,
            alpha,
        }
    }
}
