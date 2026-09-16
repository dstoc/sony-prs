use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb, Rgba};
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

fn encoded(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
    let image = ImageBuffer::from_fn(width, height, |x, y| {
        if x < width / 2 && y < height / 2 {
            Rgba([0, 0, 0, 255])
        } else {
            Rgba([255, 255, 255, 255])
        }
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
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

fn fixture_root() -> (TestRoot, FileSystemResourceProvider) {
    let root = TestRoot::new();
    fs::create_dir_all(root.path().join("book/chapters")).expect("create document directory");
    fs::create_dir_all(root.path().join("book/assets/deep")).expect("create image directory");
    fs::create_dir_all(root.path().join("shared")).expect("create shared image directory");
    fs::write(root.path().join("book/chapters/images.md"), FIXTURE).expect("write fixture");
    fs::write(
        root.path().join("book/assets/landscape.png"),
        encoded(160, 40, ImageFormat::Png),
    )
    .expect("write PNG");
    fs::write(
        root.path().join("book/assets/deep/image.webp"),
        encoded(48, 32, ImageFormat::WebP),
    )
    .expect("write WebP");
    let portrait = ImageBuffer::from_fn(30, 120, |x, y| {
        if x < 15 && y < 60 {
            Rgb([0, 0, 0])
        } else {
            Rgb([255, 255, 255])
        }
    });
    let mut portrait_bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(portrait)
        .write_to(&mut portrait_bytes, ImageFormat::Jpeg)
        .expect("encode JPEG");
    fs::write(
        root.path().join("shared/portrait.jpg"),
        portrait_bytes.into_inner(),
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
    assert_eq!(output.pixel(image_bounds.top_left), Some(0));
    assert!(output
        .pixels()
        .iter()
        .any(|pixel| *pixel > 0 && *pixel < 255));
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
