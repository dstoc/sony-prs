use image::{imageops::FilterType, ImageFormat};
use prs_markdown::harness::{render_page, HostImage, HostReader};
use prs_markdown::pagination::DisplayCommand;
use prs_markdown::resources::FileSystemResourceProvider;
use prs_markdown::style::{Insets, ReaderStyle, TextStyle};
use prs_markdown::typography::{FontConfig, FontdueTextEngine};
use prs_markdown::Reader;
use prs_markdown::Viewport;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const FIXTURE: &str = include_str!("fixtures/images.md");
const GENERATED_SOURCE: &[u8] = include_bytes!("fixtures/assets/observatory.png");

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "prs-markdown-images-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary image root");
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

fn encoded_generated(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
    let image = image::load_from_memory(GENERATED_SOURCE)
        .expect("generated image fixture should decode")
        .resize_to_fill(width, height, FilterType::Lanczos3);
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, format)
        .expect("encode test image");
    bytes.into_inner()
}

fn reader_style() -> ReaderStyle {
    ReaderStyle {
        page_padding: Insets::all(2),
        body: TextStyle::new(8, 10),
        heading: TextStyle::new(10, 12),
        code: TextStyle::new(8, 10),
        paragraph_spacing: 0,
        heading_spacing_before: 0,
        heading_spacing_after: 0,
        ..ReaderStyle::default()
    }
}

fn golden_font_engine() -> FontdueTextEngine {
    FontdueTextEngine::new(
        FontConfig::from_faces(
            notosans::REGULAR_TTF,
            notosans::BOLD_TTF,
            notosans::ITALIC_TTF,
            notosans::BOLD_ITALIC_TTF,
            notosans::REGULAR_TTF,
        ),
        512,
    )
    .expect("checked-in golden fonts should parse")
}

fn fixture_root() -> (TestRoot, FileSystemResourceProvider) {
    let root = TestRoot::new();
    fs::create_dir_all(root.path().join("book/chapters")).expect("create document directory");
    fs::create_dir_all(root.path().join("book/assets/deep")).expect("create image directory");
    fs::create_dir_all(root.path().join("shared")).expect("create shared image directory");
    fs::write(root.path().join("book/chapters/images.md"), FIXTURE).expect("write fixture");
    fs::write(
        root.path().join("book/assets/landscape.png"),
        encoded_generated(160, 40, ImageFormat::Png),
    )
    .expect("write PNG");
    fs::write(
        root.path().join("book/assets/deep/image.webp"),
        encoded_generated(48, 32, ImageFormat::WebP),
    )
    .expect("write WebP");
    fs::write(
        root.path().join("shared/portrait.jpg"),
        encoded_generated(30, 120, ImageFormat::Jpeg),
    )
    .expect("write JPEG");
    fs::write(root.path().join("book/assets/corrupt.png"), b"not a PNG")
        .expect("write corrupt image");
    let provider = FileSystemResourceProvider::new(root.path(), "book/chapters/images.md")
        .expect("create provider");
    (root, provider)
}

#[test]
fn provider_backed_harness_loads_formats_and_resolves_nested_relative_paths() {
    let (_root, provider) = fixture_root();
    let source = provider
        .read_markdown(provider.document_path())
        .expect("read Markdown fixture");
    let reader = HostReader::from_source_with_provider(
        &source,
        &provider,
        provider.document_path(),
        reader_style(),
        Viewport::new(96, 64),
        prs_markdown::layout::ApproximateTextMeasurer,
    )
    .expect("fixture should parse");

    let images = reader
        .pagination()
        .pages()
        .iter()
        .flat_map(|page| page.display_list())
        .filter_map(|command| match command {
            DisplayCommand::Image { bounds, source, .. } => Some((*bounds, source.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(images.len(), 3, "PNG, JPEG, and WebP should render");
    assert!(images
        .iter()
        .any(|(_, source)| *source == "../assets/landscape.png"));
    assert!(images
        .iter()
        .any(|(_, source)| *source == "../../shared/portrait.jpg"));
    assert!(images
        .iter()
        .any(|(_, source)| *source == "../assets/deep/image.webp"));

    let landscape = images
        .iter()
        .find(|(_, source)| *source == "../assets/landscape.png")
        .unwrap()
        .0;
    assert_eq!(
        landscape.size,
        embedded_graphics::geometry::Size::new(92, 23)
    );
    let portrait = images
        .iter()
        .find(|(_, source)| *source == "../../shared/portrait.jpg")
        .unwrap()
        .0;
    assert_eq!(
        portrait.size.height, 60,
        "portrait is constrained to content height"
    );
    assert_eq!(portrait.size.width, 15, "portrait keeps its aspect ratio");

    let fallback_text = reader
        .pagination()
        .pages()
        .iter()
        .flat_map(|page| page.display_list())
        .filter_map(|command| match command {
            DisplayCommand::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    let fallback_text = fallback_text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(fallback_text.contains("Missing image"));
    assert!(fallback_text.contains("Corrupt image"));
    assert!(
        reader.page_count() >= 3,
        "several atomic images should paginate"
    );
}

#[test]
fn loaded_images_render_as_grayscale_pixels_through_the_normal_renderer() {
    let (_root, provider) = fixture_root();
    let source = provider
        .read_markdown(provider.document_path())
        .expect("read Markdown fixture");
    let reader = HostReader::from_source_with_provider(
        &source,
        &provider,
        provider.document_path(),
        reader_style(),
        Viewport::new(96, 64),
        prs_markdown::layout::ApproximateTextMeasurer,
    )
    .expect("fixture should parse");
    let font = FontdueTextEngine::new(FontConfig::from_regular(notosans::REGULAR_TTF), 32)
        .expect("test font");
    let mut renderer = prs_markdown::EmbeddedGraphicsRenderer::new(font);
    let image_page = reader
        .pagination()
        .pages()
        .iter()
        .find(|page| {
            page.display_list()
                .iter()
                .any(|command| matches!(command, DisplayCommand::Image { .. }))
        })
        .expect("loaded image page");
    let image_bounds = image_page
        .display_list()
        .iter()
        .find_map(|command| match command {
            DisplayCommand::Image { bounds, .. } => Some(*bounds),
            _ => None,
        })
        .expect("loaded image command");
    let output: HostImage = render_page(image_page, &mut renderer);
    assert!(
        output
            .pixel(image_bounds.top_left)
            .is_some_and(|pixel| pixel < 255),
        "image should produce a non-white pixel at its top-left corner"
    );
    assert!(output
        .pixels()
        .iter()
        .any(|pixel| *pixel > 0 && *pixel < 255));
}

#[test]
fn provider_backed_image_fixture_matches_png_goldens() {
    let (_root, provider) = fixture_root();
    let source = provider
        .read_markdown(provider.document_path())
        .expect("read Markdown fixture");
    let font_engine = golden_font_engine();
    let reader = HostReader::from_source_with_provider(
        &source,
        &provider,
        provider.document_path(),
        reader_style(),
        Viewport::new(96, 64),
        font_engine.clone(),
    )
    .expect("fixture should parse");
    let mut renderer = prs_markdown::EmbeddedGraphicsRenderer::new(font_engine);

    for (page_index, page) in reader.pagination().pages().iter().enumerate() {
        let image = render_page(page, &mut renderer);
        assert_png_golden(page_index + 1, &image);
    }
}

fn assert_png_golden(page_number: usize, image: &HostImage) {
    let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/goldens/images-page-{page_number:03}.png"));
    let failure = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/prs-markdown-golden-failures/images-page-{page_number:03}.png"
    ));
    let actual = image.png_bytes();

    if std::env::var_os("PRS_MARKDOWN_UPDATE_GOLDENS").is_some() {
        fs::write(&golden, &actual)
            .unwrap_or_else(|error| panic!("write golden {}: {error}", golden.display()));
        return;
    }

    let expected = match fs::read(&golden) {
        Ok(expected) => expected,
        Err(error) => {
            write_golden_failure(&failure, &actual);
            panic!(
                "missing golden {} ({error}); rendered output was written to {}",
                golden.display(),
                failure.display()
            );
        }
    };
    if expected != actual {
        write_golden_failure(&failure, &actual);
        panic!(
            "PNG golden mismatch for images page {page_number}; rendered output was written to {}",
            failure.display()
        );
    }
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
fn retained_rasters_are_bounded_and_failed_images_do_not_abort_parsing() {
    let (_root, provider) = fixture_root();
    let source = provider
        .read_markdown(provider.document_path())
        .expect("read Markdown fixture");
    let document = prs_markdown::parse::ComrakParser::new()
        .parse(&source)
        .expect("fixture should parse");
    let images = prs_markdown::ImageResources::from_document_with_limit(
        &provider,
        provider.document_path(),
        &document,
        92,
        60,
        100,
    );
    assert!(images.retained_bytes() <= 100);
    assert!(images.contains("../assets/landscape.png"));
    assert!(images.image("../assets/landscape.png").is_none());
    assert!(images.image("../assets/missing.png").is_none());
}

#[test]
fn reader_loads_images_through_the_same_resource_boundary() {
    let (_root, provider) = fixture_root();
    let mut reader = Reader::new(provider, reader_style(), Viewport::new(96, 64));
    reader.open().expect("open image fixture");
    let image_count = (0..reader.page_count())
        .filter_map(|page| reader.pagination()?.page(page))
        .flat_map(|page| page.display_list())
        .filter(|command| matches!(command, DisplayCommand::Image { .. }))
        .count();
    assert_eq!(image_count, 3);
}
