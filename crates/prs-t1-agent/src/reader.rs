//! Thin PRS-T1 integration for the hardware-independent Markdown reader.
//!
//! This module owns the T1-specific choices around the shared reader: where a
//! development document lives, which font files to load, the page viewport
//! below the status bar, and how a physical tap becomes a reader operation.
//! Framebuffer mapping, EPDC updates, and input-device ownership remain in the
//! surrounding T1 runtime.

use crate::display::{self, CONTENT_TOP};
use crate::framebuffer::{DisplayCanvas, DisplayRegion, NativeDisplay};
use crate::refresh::{PageTone, RefreshPlan};
use embedded_graphics::geometry::Point;
use embedded_graphics::mono_font::{ascii::FONT_8X13, MonoTextStyle};
use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
use embedded_graphics::prelude::{Drawable, IntoStorage};
use embedded_graphics::text::{Baseline, Text};
use prs_markdown::geometry::Viewport;
use prs_markdown::pagination::{DisplayCommand, PageLayout};
use prs_markdown::parse::ComrakParser;
use prs_markdown::reader::{Reader, ReaderError, ReaderEvent};
use prs_markdown::render::EmbeddedGraphicsRenderer;
use prs_markdown::resources::FileSystemResourceProvider;
use prs_markdown::style::ReaderStyle;
use prs_markdown::typography::{FontConfig, FontdueTextEngine};
use prs_markdown::Color;
use std::env;
use std::fmt::Display;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const DEFAULT_DOCUMENT_ROOT: &str = "/mnt/sdcard";
pub const DEFAULT_DOCUMENT: &str = "index.md";
pub const DEFAULT_FONT: &str = "/system/fonts/DroidSans.ttf";
pub const DEFAULT_MONOSPACE_FONT: &str = "/system/fonts/HelveticaMonospacedW1G-Rg.otf";
pub const DEFAULT_MONOSPACE_BOLD_FONT: &str = "/system/fonts/HelveticaMonospacedW1G-Bd.otf";
pub const DEFAULT_MONOSPACE_ITALIC_FONT: &str = "/system/fonts/HelveticaMonospacedW1G-It.otf";
pub const DEFAULT_MONOSPACE_BOLD_ITALIC_FONT: &str =
    "/system/fonts/HelveticaMonospacedW1G-BdIt.otf";
pub const PAGE_BOTTOM_MARGIN: u32 = 16;
const GLYPH_CACHE_CAPACITY: usize = 256;

/// Runtime-selectable paths for the initial Markdown document and its fonts.
///
/// The regular face is required. Other faces fall back to the regular bytes so
/// a minimal Android font installation can still open documents; callers can
/// provide distinct files through the face-specific environment variables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderConfig {
    pub document_root: PathBuf,
    pub document: PathBuf,
    pub regular_font: PathBuf,
    pub bold_font: PathBuf,
    pub italic_font: PathBuf,
    pub bold_italic_font: PathBuf,
    pub monospace_font: PathBuf,
    pub monospace_bold_font: PathBuf,
    pub monospace_italic_font: PathBuf,
    pub monospace_bold_italic_font: PathBuf,
}

impl ReaderConfig {
    pub fn from_environment() -> Self {
        let regular_font = environment_path("PRS_T1_FONT", DEFAULT_FONT);
        Self {
            document_root: environment_path("PRS_T1_DOCUMENT_ROOT", DEFAULT_DOCUMENT_ROOT),
            document: environment_path("PRS_T1_DOCUMENT", DEFAULT_DOCUMENT),
            bold_font: environment_path("PRS_T1_FONT_BOLD", regular_font.as_os_str()),
            italic_font: environment_path("PRS_T1_FONT_ITALIC", regular_font.as_os_str()),
            bold_italic_font: environment_path("PRS_T1_FONT_BOLD_ITALIC", regular_font.as_os_str()),
            monospace_font: environment_path("PRS_T1_FONT_MONOSPACE", DEFAULT_MONOSPACE_FONT),
            monospace_bold_font: environment_path(
                "PRS_T1_FONT_MONOSPACE_BOLD",
                DEFAULT_MONOSPACE_BOLD_FONT,
            ),
            monospace_italic_font: environment_path(
                "PRS_T1_FONT_MONOSPACE_ITALIC",
                DEFAULT_MONOSPACE_ITALIC_FONT,
            ),
            monospace_bold_italic_font: environment_path(
                "PRS_T1_FONT_MONOSPACE_BOLD_ITALIC",
                DEFAULT_MONOSPACE_BOLD_ITALIC_FONT,
            ),
            regular_font,
        }
    }
}

/// The page viewport available below the T1 status bar and its breathing room.
pub fn viewport_for_display(width: u32, height: u32) -> Viewport {
    Viewport::new(
        width,
        height.saturating_sub(CONTENT_TOP as u32 + PAGE_BOTTOM_MARGIN),
    )
}

/// A T1-owned adapter around the generic Markdown reader and renderer.
pub struct T1Reader {
    reader: Reader<FileSystemResourceProvider, FontdueTextEngine, ComrakParser>,
    renderer: EmbeddedGraphicsRenderer<FontdueTextEngine>,
    external_url_notice: Option<String>,
}

impl T1Reader {
    pub fn open(config: ReaderConfig, viewport: Viewport) -> io::Result<Self> {
        if viewport.width == 0 || viewport.height == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "T1 Markdown viewport must be non-empty",
            ));
        }

        let provider = FileSystemResourceProvider::new(&config.document_root, &config.document)
            .map_err(|error| integration_error("configure Markdown resources", error))?;
        let fonts = load_fonts(&config)?;
        let engine = FontdueTextEngine::new(fonts, GLYPH_CACHE_CAPACITY)
            .map_err(|error| integration_error("load T1 Markdown fonts", error))?;
        let mut reader = Reader::with_components(
            provider,
            ComrakParser::default(),
            engine.clone(),
            ReaderStyle::default(),
            viewport,
        );
        reader
            .open()
            .map_err(|error| integration_error("open T1 Markdown document", error))?;

        Ok(Self {
            reader,
            renderer: EmbeddedGraphicsRenderer::new(engine),
            external_url_notice: None,
        })
    }

    /// Translate a whole-screen point into the page-space coordinates expected
    /// by `prs-markdown`.
    pub fn screen_to_viewport(&self, screen_point: Point) -> Option<Point> {
        let content_top = CONTENT_TOP as i32;
        if screen_point.y < content_top {
            return None;
        }
        let page_point = Point::new(screen_point.x, screen_point.y - content_top);
        self.reader
            .viewport()
            .contains(page_point)
            .then_some(page_point)
    }

    /// Translate a screen-space content tap into a shared-reader action.
    /// Links get first refusal; a blank tap on the right half advances and a
    /// blank tap on the left half goes back.
    pub fn tap(&mut self, screen_point: Point) -> Result<ReaderEvent, ReaderError> {
        let Some(page_point) = self.screen_to_viewport(screen_point) else {
            return Ok(ReaderEvent::NoAction);
        };

        let event = self.reader.activate_at(page_point)?;
        if !matches!(event, ReaderEvent::NoAction) {
            return Ok(event);
        }
        if page_point.x >= self.reader.viewport().width as i32 / 2 {
            self.reader.next_page_event()
        } else {
            self.reader.previous_page_event()
        }
    }

    pub fn next_page(&mut self) -> Result<ReaderEvent, ReaderError> {
        self.reader.next_page_event()
    }

    pub fn previous_page(&mut self) -> Result<ReaderEvent, ReaderError> {
        self.reader.previous_page_event()
    }

    pub fn back(&mut self) -> Result<ReaderEvent, ReaderError> {
        self.reader.back_event()
    }

    /// Show an application-owned fallback for an external URL. The shared
    /// reader reports the URL but deliberately does not know how to launch a
    /// browser on the T1.
    pub fn set_external_url_notice(&mut self, url: Option<String>) {
        self.external_url_notice = url;
    }

    /// Build a complete packed RGB565 screen, including the T1 status bar and
    /// the current Markdown page rendered by `prs-markdown`.
    pub fn render_frame(
        &mut self,
        status_line: &str,
        feedback: &str,
        width: u32,
        height: u32,
    ) -> io::Result<Vec<u8>> {
        let width = usize::try_from(width)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid T1 width"))?;
        let height = usize::try_from(height)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid T1 height"))?;
        let frame_len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "T1 frame is too large"))?;
        let mut frame = vec![0; frame_len];
        let mut canvas = DisplayCanvas::new(&mut frame, width, height, width * 2, 0, 0);
        canvas.fill(Rgb565::WHITE.into_storage());
        let status = [status_line.to_owned()];
        display::draw_status_bar(&mut canvas, &status);
        display::draw_reader_feedback(&mut canvas, feedback);

        let page = self.reader.current_page().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "T1 Markdown reader has no current page",
            )
        })?;
        self.renderer
            .render_at(page, &mut canvas, Point::new(0, CONTENT_TOP as i32))
            .expect("RGB565 DisplayCanvas drawing is infallible");
        self.draw_external_url_notice(&mut canvas, width, height);
        Ok(frame)
    }

    fn draw_external_url_notice(
        &self,
        canvas: &mut DisplayCanvas<'_>,
        width: usize,
        height: usize,
    ) {
        let Some(url) = self.external_url_notice.as_deref() else {
            return;
        };
        let notice_height = 24;
        let notice_top = height.saturating_sub(notice_height);
        canvas.fill_rect(
            0,
            notice_top,
            width,
            notice_height,
            Rgb565::WHITE.into_storage(),
        );

        let available_chars = width.saturating_sub(16) / FONT_8X13.character_size.width as usize;
        let mut label = String::from("External URL: ");
        label.extend(
            url.chars()
                .take(available_chars.saturating_sub(label.len())),
        );
        Text::with_baseline(
            &label,
            Point::new(8, notice_top as i32 + 5),
            MonoTextStyle::new(&FONT_8X13, Rgb565::BLACK),
            Baseline::Top,
        )
        .draw(canvas)
        .expect("RGB565 DisplayCanvas drawing is infallible");
    }

    /// Classify the current page for the T1 refresh policy.
    ///
    /// The classification is derived from the shared page's display list but
    /// remains in this adapter: the Markdown crate does not know about
    /// waveforms, damage, or EPDC scheduling. Ordinary black/white text keeps
    /// the responsive DU path; intentional gray paint and loaded rasters get
    /// a synchronous GC16 update.
    pub fn current_page_tone(&self) -> PageTone {
        self.reader
            .current_page()
            .map(page_tone)
            .unwrap_or(PageTone::Grayscale)
    }

    pub fn draw(
        &mut self,
        display: &mut NativeDisplay,
        status_line: &str,
        feedback: &str,
        refresh_region: DisplayRegion,
        plan: RefreshPlan,
    ) -> io::Result<()> {
        let frame = self.render_frame(status_line, feedback, display.width(), display.height())?;
        display.draw_frame_with_waveform(
            &frame,
            refresh_region,
            plan.waveform(),
            plan.wait_for_completion(),
            plan.force_refresh(),
        )
    }
}

fn load_fonts(config: &ReaderConfig) -> io::Result<FontConfig> {
    let regular = fs::read(&config.regular_font).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "read regular font {}: {error}",
                config.regular_font.display()
            ),
        )
    })?;
    let bold = read_optional_face(&config.bold_font, &regular);
    let italic = read_optional_face(&config.italic_font, &regular);
    let bold_italic = read_optional_face(&config.bold_italic_font, &regular);
    let monospace = read_optional_face(&config.monospace_font, &regular);
    let monospace_bold = read_optional_face(&config.monospace_bold_font, &monospace);
    let monospace_italic = read_optional_face(&config.monospace_italic_font, &monospace);
    let monospace_bold_italic = read_optional_face(&config.monospace_bold_italic_font, &monospace);
    Ok(FontConfig::from_faces_with_monospace(
        regular,
        bold,
        italic,
        bold_italic,
        monospace,
        monospace_bold,
        monospace_italic,
        monospace_bold_italic,
    ))
}

fn read_optional_face(path: &Path, fallback: &[u8]) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|_| fallback.to_vec())
}

fn environment_path(name: &str, default: impl AsRef<std::ffi::OsStr>) -> PathBuf {
    env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default.as_ref()))
}

fn integration_error(stage: &str, error: impl Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{stage}: {error}"))
}

fn page_tone(page: &PageLayout) -> PageTone {
    if page.display_list().iter().any(command_requires_grayscale) {
        PageTone::Grayscale
    } else {
        PageTone::Monochrome
    }
}

fn command_requires_grayscale(command: &DisplayCommand) -> bool {
    match command {
        DisplayCommand::Text { style, .. } => style.ink != 0 && style.ink != u8::MAX,
        DisplayCommand::Fill { style, .. } => color_requires_grayscale(style.color),
        DisplayCommand::Border { style, .. } | DisplayCommand::Rule { style, .. } => {
            color_requires_grayscale(style.color)
        }
        DisplayCommand::Image { .. } => true,
        DisplayCommand::ImagePlaceholder { .. } => false,
    }
}

fn color_requires_grayscale(color: Color) -> bool {
    color.alpha != 0
        && !((color.red == 0 && color.green == 0 && color.blue == 0)
            || (color.red == u8::MAX && color.green == u8::MAX && color.blue == u8::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::geometry::Point;
    use prs_markdown::typography::{FontFace, TextEngine, TextRun, TextStyle as TypographyStyle};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_config(root: &Path) -> ReaderConfig {
        let font = PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf");
        let monospace = PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf");
        let monospace_bold =
            PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf");
        let monospace_italic =
            PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Oblique.ttf");
        let monospace_bold_italic =
            PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-BoldOblique.ttf");
        ReaderConfig {
            document_root: root.to_owned(),
            document: PathBuf::from("index.md"),
            regular_font: font.clone(),
            bold_font: font.clone(),
            italic_font: font.clone(),
            bold_italic_font: font,
            monospace_font: monospace,
            monospace_bold_font: monospace_bold,
            monospace_italic_font: monospace_italic,
            monospace_bold_italic_font: monospace_bold_italic,
        }
    }

    fn fixture_root(source: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock should be after the Unix epoch")
            .as_nanos();
        let root = env::temp_dir().join(format!("prs-t1-reader-{suffix}"));
        fs::create_dir_all(&root).expect("create reader fixture root");
        fs::write(root.join("index.md"), source).expect("write reader fixture");
        root
    }

    fn raster_for_face(engine: &mut FontdueTextEngine, face: FontFace) -> Vec<u8> {
        let layout = engine.layout(&[TextRun::new("Ag", TypographyStyle::new(face, 20))], 100);
        TextEngine::rasterize_glyph(engine, &layout.glyphs[0]).alpha
    }

    #[test]
    fn configured_monospace_faces_load_and_missing_variants_use_monospace_regular() {
        let root = fixture_root("```rust\nfn main() {}\n```");
        let config = fixture_config(&root);
        let mut engine =
            FontdueTextEngine::new(load_fonts(&config).expect("load fixture fonts"), 16)
                .expect("fixture fonts should parse");
        let faces = [
            FontFace::Monospace,
            FontFace::MonospaceBold,
            FontFace::MonospaceItalic,
            FontFace::MonospaceBoldItalic,
        ];
        let rasters = faces
            .into_iter()
            .map(|face| raster_for_face(&mut engine, face))
            .collect::<Vec<_>>();
        for (index, raster) in rasters.iter().enumerate() {
            assert!(
                rasters[..index].iter().all(|previous| previous != raster),
                "configured monospace face {index} should be distinct"
            );
        }

        let mut fallback_config = config;
        fallback_config.monospace_bold_font = root.join("missing-monospace-bold.ttf");
        let mut fallback_engine = FontdueTextEngine::new(
            load_fonts(&fallback_config).expect("fallback fonts should load"),
            16,
        )
        .expect("fallback fonts should parse");
        assert_eq!(
            raster_for_face(&mut fallback_engine, FontFace::Monospace),
            raster_for_face(&mut fallback_engine, FontFace::MonospaceBold)
        );
        fs::remove_dir_all(root).expect("remove reader fixture root");
    }

    #[test]
    fn configured_document_renders_below_the_status_bar() {
        let root = fixture_root("# Development reader\n\nThis is a real page.");
        let viewport = Viewport::new(240, 180);
        let mut reader = T1Reader::open(fixture_config(&root), viewport).expect("open fixture");
        let frame = reader
            .render_frame("100%|On|On|On|||12:00", "Opened document", 240, 256)
            .expect("render fixture");

        let content_offset = CONTENT_TOP * 240 * 2;
        assert!(frame[content_offset..]
            .iter()
            .any(|pixel| *pixel != Rgb565::WHITE.into_storage().to_ne_bytes()[0]));
        assert_eq!(reader.reader.page_count(), 1);
        assert_eq!(reader.current_page_tone(), PageTone::Monochrome);
        fs::remove_dir_all(root).expect("remove reader fixture root");
    }

    #[test]
    fn gray_display_list_commands_request_quality_waveform() {
        let mut page = PageLayout::new(1, Viewport::new(100, 80));
        page.push_command(DisplayCommand::Fill {
            bounds: embedded_graphics::primitives::Rectangle::new(
                Point::zero(),
                embedded_graphics::geometry::Size::new(20, 20),
            ),
            style: prs_markdown::FillStyle::new(Color::rgb(240, 240, 240)),
        });

        assert_eq!(page_tone(&page), PageTone::Grayscale);
    }

    #[test]
    fn loaded_images_request_quality_waveform_even_without_gray_decorations() {
        let mut page = PageLayout::new(1, Viewport::new(100, 80));
        page.push_command(DisplayCommand::Image {
            bounds: embedded_graphics::primitives::Rectangle::new(
                Point::zero(),
                embedded_graphics::geometry::Size::new(1, 1),
            ),
            image: prs_markdown::RasterImage::new(1, 1, std::sync::Arc::<[u8]>::from(vec![128]))
                .expect("valid test raster"),
            source: "image.png".into(),
            alt: "test image".into(),
        });

        assert_eq!(page_tone(&page), PageTone::Grayscale);
    }

    #[test]
    fn blank_content_tap_advances_and_goes_back_by_page() {
        let root = fixture_root(&"line\n".repeat(80));
        let viewport = Viewport::new(240, 120);
        let mut reader = T1Reader::open(fixture_config(&root), viewport).expect("open fixture");
        assert!(reader.reader.page_count() > 1);

        let event = reader
            .tap(Point::new(220, CONTENT_TOP as i32 + 60))
            .expect("advance page");
        assert_eq!(
            event,
            ReaderEvent::PageChanged {
                page: 1,
                page_count: reader.reader.page_count()
            }
        );

        let event = reader
            .tap(Point::new(20, CONTENT_TOP as i32 + 60))
            .expect("go back page");
        assert_eq!(
            event,
            ReaderEvent::PageChanged {
                page: 0,
                page_count: reader.reader.page_count()
            }
        );
        fs::remove_dir_all(root).expect("remove reader fixture root");
    }

    #[test]
    fn screen_coordinates_translate_to_viewport_coordinates() {
        let root = fixture_root("reader");
        let reader =
            T1Reader::open(fixture_config(&root), Viewport::new(240, 120)).expect("open fixture");

        assert_eq!(
            reader.screen_to_viewport(Point::new(23, CONTENT_TOP as i32 + 11)),
            Some(Point::new(23, 11))
        );
        assert_eq!(reader.screen_to_viewport(Point::new(23, 47)), None);
        assert_eq!(
            reader.screen_to_viewport(Point::new(240, CONTENT_TOP as i32)),
            None
        );

        fs::remove_dir_all(root).expect("remove reader fixture root");
    }
}
