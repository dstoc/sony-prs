//! Thin PRS-T1 integration for the hardware-independent Markdown reader.
//!
//! This module owns the T1-specific choices around the shared reader: where a
//! development document lives, which font files to load, the page viewport
//! below the status bar, and how a physical tap becomes a reader operation.
//! Framebuffer mapping, EPDC updates, and input-device ownership remain in the
//! surrounding T1 runtime.

use crate::display::{self, CONTENT_TOP};
use crate::framebuffer::{DisplayCanvas, DisplayRegion, NativeDisplay, WaveformMode};
use embedded_graphics::geometry::Point;
use embedded_graphics::mono_font::{ascii::FONT_8X13, MonoTextStyle};
use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
use embedded_graphics::prelude::{Drawable, IntoStorage};
use embedded_graphics::text::{Baseline, Text};
use prs_markdown::geometry::Viewport;
use prs_markdown::parse::ComrakParser;
use prs_markdown::reader::{Reader, ReaderError, ReaderEvent};
use prs_markdown::render::EmbeddedGraphicsRenderer;
use prs_markdown::resources::FileSystemResourceProvider;
use prs_markdown::style::ReaderStyle;
use prs_markdown::typography::{FontConfig, FontdueTextEngine};
use std::env;
use std::fmt::Display;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const DEFAULT_DOCUMENT_ROOT: &str = "/mnt/sdcard";
pub const DEFAULT_DOCUMENT: &str = "index.md";
pub const DEFAULT_FONT: &str = "/system/fonts/DroidSans.ttf";
pub const DEFAULT_MONOSPACE_FONT: &str = "/system/fonts/DroidSansMono.ttf";
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

    pub fn draw(
        &mut self,
        display: &mut NativeDisplay,
        status_line: &str,
        refresh_region: DisplayRegion,
        waveform: WaveformMode,
        wait_for_completion: bool,
        force_refresh: bool,
    ) -> io::Result<()> {
        let frame = self.render_frame(status_line, display.width(), display.height())?;
        display.draw_frame_with_waveform(
            &frame,
            refresh_region,
            waveform,
            wait_for_completion,
            force_refresh,
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
    Ok(FontConfig::from_faces(
        regular,
        bold,
        italic,
        bold_italic,
        monospace,
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

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::geometry::Point;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_config(root: &Path) -> ReaderConfig {
        let font = PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf");
        let monospace = PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf");
        ReaderConfig {
            document_root: root.to_owned(),
            document: PathBuf::from("index.md"),
            regular_font: font.clone(),
            bold_font: font.clone(),
            italic_font: font.clone(),
            bold_italic_font: font,
            monospace_font: monospace,
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

    #[test]
    fn configured_document_renders_below_the_status_bar() {
        let root = fixture_root("# Development reader\n\nThis is a real page.");
        let viewport = Viewport::new(240, 180);
        let mut reader = T1Reader::open(fixture_config(&root), viewport).expect("open fixture");
        let frame = reader
            .render_frame("100%|On|On|On|||12:00", 240, 256)
            .expect("render fixture");

        let content_offset = CONTENT_TOP * 240 * 2;
        assert!(frame[content_offset..]
            .iter()
            .any(|pixel| *pixel != Rgb565::WHITE.into_storage().to_ne_bytes()[0]));
        assert_eq!(reader.reader.page_count(), 1);
        fs::remove_dir_all(root).expect("remove reader fixture root");
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
