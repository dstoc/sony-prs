//! Thin PRS-T1 integration for the hardware-independent Markdown reader.
//!
//! This module owns the T1-specific choices around the shared reader: where a
//! development document lives, which font files to load, the page viewport
//! below the status bar, and how a physical tap becomes a reader operation.
//! Framebuffer mapping, EPDC updates, and input-device ownership remain in the
//! surrounding T1 runtime.

use crate::display::{self, CONTENT_TOP};
use crate::framebuffer::{DisplayCanvas, DisplayRegion, NativeDisplay};
use crate::qr::QrMatrix;
use crate::refresh::{PageTone, RefreshPlan};
use embedded_graphics::geometry::Point;
use embedded_graphics::mono_font::{
    ascii::{FONT_8X13, FONT_8X13_BOLD},
    MonoTextStyle,
};
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
const EXTERNAL_LINK_OVERLAY_WIDTH: usize = 360;
const EXTERNAL_LINK_OVERLAY_HEIGHT: usize = 214;
const EXTERNAL_LINK_OVERLAY_MARGIN: usize = 12;
const EXTERNAL_LINK_QR_BOX_SIZE: usize = 176;
const EXTERNAL_LINK_QR_QUIET_ZONE: usize = 4;

/// The damage region used for the transient external-link overlay.
pub(crate) fn external_link_overlay_region(width: u32, height: u32) -> DisplayRegion {
    let width = width as usize;
    let height = height as usize;
    let overlay_width = EXTERNAL_LINK_OVERLAY_WIDTH.min(width);
    let overlay_height =
        EXTERNAL_LINK_OVERLAY_HEIGHT.min(height.saturating_sub(PAGE_BOTTOM_MARGIN as usize));
    let right_margin = EXTERNAL_LINK_OVERLAY_MARGIN.min(width.saturating_sub(overlay_width));
    let bottom = height
        .saturating_sub(PAGE_BOTTOM_MARGIN as usize)
        .min(height);
    let top = bottom.saturating_sub(overlay_height);
    DisplayRegion::new(
        width
            .saturating_sub(right_margin)
            .saturating_sub(overlay_width) as u32,
        top as u32,
        overlay_width as u32,
        overlay_height as u32,
    )
}

fn truncated_url_lines(url: &str, chars_per_line: usize, max_lines: usize) -> Vec<String> {
    if chars_per_line == 0 || max_lines == 0 {
        return Vec::new();
    }

    let mut remaining = url.chars();
    let mut lines = Vec::new();
    for line_index in 0..max_lines {
        let mut line = remaining.by_ref().take(chars_per_line).collect::<String>();
        let has_more = remaining.next().is_some();
        if has_more && line_index + 1 == max_lines {
            let suffix = "...";
            line = line
                .chars()
                .take(chars_per_line.saturating_sub(suffix.len()))
                .collect();
            line.push_str(suffix);
        }
        if line.is_empty() {
            break;
        }
        lines.push(line);
        if !has_more {
            break;
        }
    }
    lines
}

#[derive(Clone, Debug)]
struct ExternalLinkOverlay {
    url: String,
    qr: QrMatrix,
}

impl ExternalLinkOverlay {
    fn new(url: String) -> io::Result<Self> {
        let qr = QrMatrix::encode(&url).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("encode external URL as QR: {error}"),
            )
        })?;
        Ok(Self { url, qr })
    }
}

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

    /// Build a reader configuration from the atomically published bundle.
    ///
    /// The manifest is the only bundle metadata the UI needs.  Keeping this
    /// lookup in the T1 adapter means the Markdown crate remains unaware of
    /// PRSync, revisions, or transport details.
    pub fn from_current_bundle() -> io::Result<Self> {
        let library_root =
            environment_path("PRS_T1_LIBRARY_ROOT", crate::sync::DEFAULT_LIBRARY_ROOT);
        Self::from_library_root(library_root)
    }

    pub fn from_library_root(library_root: impl AsRef<Path>) -> io::Result<Self> {
        let library_root = library_root.as_ref();
        let current = library_root.join("current");
        let manifest_path = current.join("manifest.json");
        let manifest = fs::read_to_string(&manifest_path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "read current bundle manifest {}: {error}",
                    manifest_path.display()
                ),
            )
        })?;
        let manifest: prs_sync_protocol::Manifest =
            serde_json::from_str(&manifest).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("parse current bundle manifest: {error}"),
                )
            })?;
        let mut config = Self::from_environment();
        config.document_root = current;
        config.document = PathBuf::from(manifest.entry_point.as_str());
        Ok(config)
    }

    fn copy_font_paths_from(&mut self, source: &Self) {
        self.regular_font = source.regular_font.clone();
        self.bold_font = source.bold_font.clone();
        self.italic_font = source.italic_font.clone();
        self.bold_italic_font = source.bold_italic_font.clone();
        self.monospace_font = source.monospace_font.clone();
        self.monospace_bold_font = source.monospace_bold_font.clone();
        self.monospace_italic_font = source.monospace_italic_font.clone();
        self.monospace_bold_italic_font = source.monospace_bold_italic_font.clone();
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
    config: ReaderConfig,
    entry_point: PathBuf,
    library_root: PathBuf,
    external_link_overlay: Option<ExternalLinkOverlay>,
    library_empty: bool,
}

impl T1Reader {
    pub fn open(config: ReaderConfig, viewport: Viewport) -> io::Result<Self> {
        let library_root = library_root_for_config(&config);
        Self::open_with_library_root(config, viewport, library_root)
    }

    /// Open a document while keeping the PRSync publication root separate
    /// from the document used as the startup placeholder.
    pub(crate) fn open_with_library_root(
        config: ReaderConfig,
        viewport: Viewport,
        library_root: impl AsRef<Path>,
    ) -> io::Result<Self> {
        if viewport.width == 0 || viewport.height == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "T1 Markdown viewport must be non-empty",
            ));
        }

        let provider = FileSystemResourceProvider::new(&config.document_root, &config.document)
            .map_err(|error| integration_error("configure Markdown resources", error))?;
        let reload_config = config.clone();
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
            config: reload_config,
            entry_point: config.document,
            library_root: library_root.as_ref().to_owned(),
            external_link_overlay: None,
            library_empty: false,
        })
    }

    /// Reopen the current bundle after an atomic publication.
    ///
    /// The filesystem provider canonicalizes the generation behind `current`,
    /// so an active reader keeps using its old complete generation until this
    /// method is called at an idle boundary.
    pub fn reload_current_bundle(&mut self) -> io::Result<()> {
        let viewport = self.reader.viewport();
        let mut config = match ReaderConfig::from_library_root(&self.library_root) {
            Ok(config) => config,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.library_empty = true;
                self.external_link_overlay = None;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        config.copy_font_paths_from(&self.config);
        let previous = (!self.library_empty)
            .then(|| self.reader.current_location().cloned())
            .flatten();
        let mut replacement = Self::open_with_library_root(config, viewport, &self.library_root)?;
        if let Some(location) = previous {
            if replacement
                .reader
                .open_document(Path::new(location.document.as_ref()))
                .is_ok()
            {
                if let Some(anchor) = location.anchor.as_deref() {
                    let _ = replacement.reader.navigate_to_anchor(anchor);
                }
            }
        }
        *self = replacement;
        Ok(())
    }

    pub fn is_library_empty(&self) -> bool {
        self.library_empty
    }

    /// Return to the entry point of the currently open bundle.
    pub fn return_to_entry_point(&mut self) -> Result<ReaderEvent, ReaderError> {
        if self.library_empty {
            return Err(ReaderError::NoDocumentOpen);
        }
        self.reader.open_document(&self.entry_point)
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
        if self.library_empty {
            return Err(ReaderError::NoDocumentOpen);
        }
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
        if self.library_empty {
            return Err(ReaderError::NoDocumentOpen);
        }
        self.reader.next_page_event()
    }

    pub fn previous_page(&mut self) -> Result<ReaderEvent, ReaderError> {
        if self.library_empty {
            return Err(ReaderError::NoDocumentOpen);
        }
        self.reader.previous_page_event()
    }

    pub fn back(&mut self) -> Result<ReaderEvent, ReaderError> {
        if self.library_empty {
            return Err(ReaderError::NoDocumentOpen);
        }
        self.reader.back_event()
    }

    /// Show an application-owned QR overlay for an external URL. The shared
    /// reader reports the URL but deliberately does not launch a browser or
    /// perform network I/O on the T1.
    pub fn set_external_link_overlay(&mut self, url: String) -> io::Result<()> {
        self.external_link_overlay = Some(ExternalLinkOverlay::new(url)?);
        Ok(())
    }

    /// Dismiss the transient external-link overlay before the next input is
    /// handled. The caller uses the return value to request overlay damage.
    pub fn clear_external_link_overlay(&mut self) -> bool {
        self.external_link_overlay.take().is_some()
    }

    /// Build a complete packed RGB565 screen, including the T1 status bar and
    /// the current Markdown page rendered by `prs-markdown`.
    pub fn render_frame(
        &mut self,
        status_line: &str,
        feedback: Option<&str>,
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
        if let Some(feedback) = feedback {
            display::draw_reader_feedback(&mut canvas, feedback);
        }

        let page = self.reader.current_page().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "T1 Markdown reader has no current page",
            )
        })?;
        self.renderer
            .render_at(page, &mut canvas, Point::new(0, CONTENT_TOP as i32))
            .expect("RGB565 DisplayCanvas drawing is infallible");
        self.draw_external_link_overlay(&mut canvas, width, height);
        Ok(frame)
    }

    fn draw_external_link_overlay(
        &self,
        canvas: &mut DisplayCanvas<'_>,
        width: usize,
        height: usize,
    ) {
        let Some(overlay) = self.external_link_overlay.as_ref() else {
            return;
        };
        let region = external_link_overlay_region(width as u32, height as u32);
        let left = region.left as usize;
        let top = region.top as usize;
        let panel_width = region.width as usize;
        let panel_height = region.height as usize;
        if panel_width == 0 || panel_height == 0 {
            return;
        }
        canvas.fill_rect(
            left,
            top,
            panel_width,
            panel_height,
            Rgb565::WHITE.into_storage(),
        );
        canvas.stroke_rect(
            left,
            top,
            panel_width,
            panel_height,
            Rgb565::BLACK.into_storage(),
        );

        let qr_box_size = EXTERNAL_LINK_QR_BOX_SIZE
            .min(panel_width.saturating_sub(EXTERNAL_LINK_OVERLAY_MARGIN * 2))
            .min(panel_height.saturating_sub(EXTERNAL_LINK_OVERLAY_MARGIN * 2));
        let module_count = overlay
            .qr
            .size()
            .saturating_add(EXTERNAL_LINK_QR_QUIET_ZONE * 2);
        let scale = qr_box_size.checked_div(module_count).unwrap_or(0);
        if scale == 0 {
            return;
        }
        let qr_size = module_count.saturating_mul(scale);
        let qr_left = left
            .saturating_add(panel_width)
            .saturating_sub(EXTERNAL_LINK_OVERLAY_MARGIN)
            .saturating_sub(qr_size);
        let qr_top = top.saturating_add(panel_height.saturating_sub(qr_size) / 2);
        for y in 0..overlay.qr.size() {
            for x in 0..overlay.qr.size() {
                if overlay.qr.is_dark(x, y) {
                    canvas.fill_rect(
                        qr_left.saturating_add((x + EXTERNAL_LINK_QR_QUIET_ZONE) * scale),
                        qr_top.saturating_add((y + EXTERNAL_LINK_QR_QUIET_ZONE) * scale),
                        scale,
                        scale,
                        Rgb565::BLACK.into_storage(),
                    );
                }
            }
        }

        let text_left = left.saturating_add(EXTERNAL_LINK_OVERLAY_MARGIN);
        let text_right = qr_left.saturating_sub(EXTERNAL_LINK_OVERLAY_MARGIN);
        let text_width = text_right.saturating_sub(text_left);
        let chars_per_line = text_width / FONT_8X13.character_size.width as usize;
        Text::with_baseline(
            "External link",
            Point::new(text_left as i32, top.saturating_add(14) as i32),
            MonoTextStyle::new(&FONT_8X13_BOLD, Rgb565::BLACK),
            Baseline::Top,
        )
        .draw(canvas)
        .expect("RGB565 DisplayCanvas drawing is infallible");
        Text::with_baseline(
            "URL:",
            Point::new(text_left as i32, top.saturating_add(36) as i32),
            MonoTextStyle::new(&FONT_8X13, Rgb565::BLACK),
            Baseline::Top,
        )
        .draw(canvas)
        .expect("RGB565 DisplayCanvas drawing is infallible");

        let line_height = FONT_8X13.character_size.height as usize + 2;
        let max_lines = panel_height
            .saturating_sub(56)
            .checked_div(line_height)
            .unwrap_or(0);
        for (index, line) in truncated_url_lines(&overlay.url, chars_per_line, max_lines)
            .into_iter()
            .enumerate()
        {
            Text::with_baseline(
                &line,
                Point::new(
                    text_left as i32,
                    top.saturating_add(54).saturating_add(index * line_height) as i32,
                ),
                MonoTextStyle::new(&FONT_8X13, Rgb565::BLACK),
                Baseline::Top,
            )
            .draw(canvas)
            .expect("RGB565 DisplayCanvas drawing is infallible");
        }
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
        feedback: Option<&str>,
        refresh_region: DisplayRegion,
        plan: RefreshPlan,
    ) -> io::Result<()> {
        if self.library_empty {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no current Markdown bundle",
            ));
        }
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

fn library_root_for_config(config: &ReaderConfig) -> PathBuf {
    if config
        .document_root
        .file_name()
        .is_some_and(|name| name == "current")
    {
        config
            .document_root
            .parent()
            .map(Path::to_owned)
            .unwrap_or_else(|| config.document_root.clone())
    } else {
        config.document_root.clone()
    }
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
        DisplayCommand::TaskCheckbox { .. } => false,
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
    use std::env;
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_config(root: &Path) -> ReaderConfig {
        let paths = [
            ("regular.ttf", notosans::REGULAR_TTF),
            ("bold.ttf", notosans::BOLD_TTF),
            ("italic.ttf", notosans::ITALIC_TTF),
            ("bold-italic.ttf", notosans::BOLD_ITALIC_TTF),
            ("monospace.ttf", notosans::REGULAR_TTF),
            ("monospace-bold.ttf", notosans::BOLD_TTF),
            ("monospace-italic.ttf", notosans::ITALIC_TTF),
            ("monospace-bold-italic.ttf", notosans::BOLD_ITALIC_TTF),
        ]
        .map(|(name, bytes)| {
            let path = root.join(name);
            fs::write(&path, bytes).expect("write checked-in fixture font");
            path
        });
        ReaderConfig {
            document_root: root.to_owned(),
            document: PathBuf::from("index.md"),
            regular_font: paths[0].clone(),
            bold_font: paths[1].clone(),
            italic_font: paths[2].clone(),
            bold_italic_font: paths[3].clone(),
            monospace_font: paths[4].clone(),
            monospace_bold_font: paths[5].clone(),
            monospace_italic_font: paths[6].clone(),
            monospace_bold_italic_font: paths[7].clone(),
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

    fn assert_png_golden(name: &str, actual: &[u8]) {
        let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/goldens")
            .join(format!("{name}.png"));
        if env::var_os("PRS_T1_UPDATE_GOLDENS").is_some() {
            fs::create_dir_all(golden.parent().expect("golden has a parent"))
                .expect("create reader screenshot golden directory");
            fs::write(&golden, actual).expect("write reader screenshot golden");
            return;
        }

        let expected = fs::read(&golden).unwrap_or_else(|error| {
            let failure = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target")
                .join("prs-t1-agent-golden-failures")
                .join(format!("{name}.png"));
            fs::create_dir_all(failure.parent().expect("failure has a parent"))
                .expect("create screenshot failure directory");
            fs::write(&failure, actual).expect("write screenshot failure");
            panic!(
                "missing native reader golden {} ({error}); rendered output was written to {}",
                golden.display(),
                failure.display()
            );
        });
        if expected != actual {
            let failure = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target")
                .join("prs-t1-agent-golden-failures")
                .join(format!("{name}.png"));
            fs::create_dir_all(
                failure
                    .parent()
                    .expect("create screenshot failure directory"),
            )
            .expect("create screenshot failure directory");
            fs::write(&failure, actual).expect("write screenshot failure");
            panic!(
                "native reader golden mismatch for {name}; rendered output was written to {}",
                failure.display()
            );
        }
    }

    fn sync_library_root() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock should be after the Unix epoch")
            .as_nanos();
        let root = env::temp_dir().join(format!("prs-t1-sync-library-{suffix}"));
        fs::create_dir_all(&root).expect("create sync library root");
        root
    }

    fn publish_test_bundle(root: &Path, generation: &str, entry_point: &str, content: &str) {
        use std::os::unix::fs::symlink;

        let generation_root = root.join(generation);
        fs::create_dir_all(&generation_root).expect("create bundle generation");
        fs::write(generation_root.join(entry_point), content).expect("write bundle entry point");
        fs::write(
            generation_root.join("manifest.json"),
            format!(
                r#"{{"protocol_version":{{"major":1,"minor":0}},"bundle_format_version":1,"entry_point":"{entry_point}","files":[{{"path":"{entry_point}","size":{}}}]}}"#,
                content.len()
            ),
        )
        .expect("write bundle manifest");
        symlink(generation, root.join("current")).expect("publish current bundle");
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
            .render_frame("100%|On|On|On|||12:00", None, 240, 256)
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
    fn reader_without_feedback_matches_png_golden() {
        let root = fixture_root("# Native reader\n\nNormal reading stays quiet.");
        let mut reader =
            T1Reader::open(fixture_config(&root), Viewport::new(600, 708)).expect("open fixture");
        let frame = reader
            .render_frame("87%|UP|ON|ON||12:34", None, 600, 800)
            .expect("render normal reader");
        let png = crate::display::rgb565_to_png(&frame, 600, 800).expect("encode reader PNG");
        assert_png_golden("reader-normal", &png);
        fs::remove_dir_all(root).expect("remove reader fixture root");
    }

    #[test]
    fn reader_error_feedback_matches_png_golden() {
        let root = fixture_root("# Native reader\n\nA recoverable error is visible.");
        let mut reader =
            T1Reader::open(fixture_config(&root), Viewport::new(600, 708)).expect("open fixture");
        let frame = reader
            .render_frame(
                "87%|UP|ON|ON||12:34",
                Some("Synchronization failed"),
                600,
                800,
            )
            .expect("render reader error");
        let png = crate::display::rgb565_to_png(&frame, 600, 800).expect("encode reader PNG");
        assert_png_golden("reader-error-feedback", &png);
        fs::remove_dir_all(root).expect("remove reader fixture root");
    }

    #[test]
    fn external_link_overlay_matches_png_golden() {
        const URL: &str = "https://example.com/reader";
        let root = fixture_root(&format!("# Native reader\n\n[External link]({URL})\n"));
        let mut reader =
            T1Reader::open(fixture_config(&root), Viewport::new(600, 708)).expect("open fixture");
        let link = reader
            .reader
            .current_page()
            .expect("current page")
            .hit_regions
            .first()
            .expect("external link hit region")
            .bounds
            .top_left;
        let event = reader
            .tap(Point::new(link.x, link.y + CONTENT_TOP as i32))
            .expect("activate external link");
        assert_eq!(event, ReaderEvent::ExternalUrl(URL.into()));
        reader
            .set_external_link_overlay(URL.into())
            .expect("encode external URL QR");

        let frame = reader
            .render_frame("87%|UP|ON|ON||12:34", None, 600, 800)
            .expect("render external-link overlay");
        let png = crate::display::rgb565_to_png(&frame, 600, 800).expect("encode reader PNG");
        assert_png_golden("reader-external-link", &png);
        fs::remove_dir_all(root).expect("remove reader fixture root");
    }

    #[test]
    fn external_link_overlay_is_replaced_and_cleared_without_navigation() {
        const FIRST_URL: &str = "https://example.com/first";
        const SECOND_URL: &str = "https://example.org/second";
        let root = fixture_root(&format!(
            "# Native reader\n\n[First]({FIRST_URL})\n\n[Second]({SECOND_URL})\n"
        ));
        let mut reader =
            T1Reader::open(fixture_config(&root), Viewport::new(600, 708)).expect("open fixture");
        let first_link = reader
            .reader
            .current_page()
            .expect("current page")
            .hit_regions[0]
            .bounds
            .top_left;
        let first_event = reader
            .tap(Point::new(first_link.x, first_link.y + CONTENT_TOP as i32))
            .expect("activate first external link");
        assert_eq!(first_event, ReaderEvent::ExternalUrl(FIRST_URL.into()));
        reader
            .set_external_link_overlay(FIRST_URL.into())
            .expect("encode first external URL QR");
        assert_eq!(
            reader
                .external_link_overlay
                .as_ref()
                .map(|overlay| overlay.url.as_str()),
            Some(FIRST_URL)
        );
        assert_eq!(
            reader
                .external_link_overlay
                .as_ref()
                .map(|overlay| &overlay.qr),
            Some(&QrMatrix::encode(FIRST_URL).expect("encode first URL for test"))
        );

        reader
            .set_external_link_overlay(SECOND_URL.into())
            .expect("replace external URL QR");
        assert_eq!(
            reader
                .external_link_overlay
                .as_ref()
                .map(|overlay| overlay.url.as_str()),
            Some(SECOND_URL)
        );
        assert_eq!(
            reader
                .external_link_overlay
                .as_ref()
                .map(|overlay| &overlay.qr),
            Some(&QrMatrix::encode(SECOND_URL).expect("encode second URL for test"))
        );
        assert_eq!(
            reader.reader.current_location().unwrap().document.as_ref(),
            "index.md"
        );
        assert_eq!(reader.reader.history().len(), 1);

        assert!(reader.clear_external_link_overlay());
        assert!(!reader.clear_external_link_overlay());
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

    #[test]
    fn current_bundle_manifest_selects_its_entry_point() {
        use std::os::unix::fs::symlink;

        let root = env::temp_dir().join(format!(
            "prs-t1-current-bundle-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock should be after the Unix epoch")
                .as_nanos()
        ));
        let generation = root.join(".generation");
        fs::create_dir_all(&generation).expect("create bundle generation");
        fs::write(generation.join("chapter.md"), "# Chapter").expect("write entry point");
        fs::write(
            generation.join("manifest.json"),
            r#"{"protocol_version":{"major":1,"minor":0},"bundle_format_version":1,"entry_point":"chapter.md","files":[{"path":"chapter.md","size":9}]}"#,
        )
        .expect("write bundle manifest");
        symlink(".generation", root.join("current")).expect("publish current symlink");

        let config = ReaderConfig::from_library_root(&root).expect("read current bundle");
        assert_eq!(config.document_root, root.join("current"));
        assert_eq!(config.document, PathBuf::from("chapter.md"));
        fs::remove_dir_all(root).expect("remove bundle fixture");
    }

    #[test]
    fn reload_adopts_an_empty_bundle_after_current_is_cleared() {
        use std::os::unix::fs::symlink;

        let root = fixture_root("# Current bundle");
        let generation = root.join(".prs-sync-library-old");
        fs::create_dir_all(&generation).expect("create bundle generation");
        fs::write(generation.join("index.md"), "# Current bundle").expect("write entry point");
        fs::write(
            generation.join("manifest.json"),
            r#"{"protocol_version":{"major":1,"minor":0},"bundle_format_version":1,"entry_point":"index.md","files":[{"path":"index.md","size":16}]}"#,
        )
        .expect("write bundle manifest");
        fs::remove_file(root.join("index.md")).expect("remove fixture document");
        symlink(".prs-sync-library-old", root.join("current")).expect("publish current symlink");

        let mut reader = T1Reader::open(
            fixture_config(&root.join("current")),
            Viewport::new(240, 120),
        )
        .expect("open current bundle");
        fs::remove_file(root.join("current")).expect("clear current symlink");

        reader
            .reload_current_bundle()
            .expect("adopt cleared bundle");

        assert!(reader.is_library_empty());
        assert!(
            generation.exists(),
            "retired generation stays until cleanup"
        );
        fs::remove_dir_all(root).expect("remove bundle fixture");
    }

    #[test]
    fn reload_adopts_sync_bundle_when_startup_uses_a_placeholder() {
        let placeholder_root = fixture_root("# Placeholder");
        let library_root = sync_library_root();
        let mut reader = T1Reader::open_with_library_root(
            fixture_config(&placeholder_root),
            Viewport::new(240, 120),
            &library_root,
        )
        .expect("open placeholder reader");

        publish_test_bundle(&library_root, ".generation-one", "chapter.md", "# Chapter");
        reader
            .reload_current_bundle()
            .expect("adopt first published bundle");
        assert!(!reader.is_library_empty());
        assert_eq!(reader.entry_point, PathBuf::from("chapter.md"));
        assert_eq!(
            reader
                .reader
                .current_location()
                .expect("first document location")
                .document
                .as_ref(),
            "chapter.md"
        );

        fs::remove_file(library_root.join("current")).expect("remove first current link");
        publish_test_bundle(
            &library_root,
            ".generation-two",
            "new-entry.md",
            "# New entry",
        );
        reader
            .reload_current_bundle()
            .expect("adopt second published bundle");
        assert_eq!(reader.entry_point, PathBuf::from("new-entry.md"));
        assert_eq!(
            reader
                .reader
                .current_location()
                .expect("second document location")
                .document
                .as_ref(),
            "new-entry.md"
        );

        fs::remove_dir_all(placeholder_root).expect("remove placeholder fixture");
        fs::remove_dir_all(library_root).expect("remove sync library fixture");
    }

    #[test]
    fn failed_bundle_reload_keeps_the_previously_active_document() {
        let library_root = sync_library_root();
        publish_test_bundle(&library_root, ".generation-good", "old.md", "# Old");
        let mut config = fixture_config(&library_root.join("current"));
        config.document = PathBuf::from("old.md");
        let mut reader =
            T1Reader::open_with_library_root(config, Viewport::new(240, 120), &library_root)
                .expect("open good bundle");

        fs::remove_file(library_root.join("current")).expect("remove good current link");
        publish_test_bundle(&library_root, ".generation-broken", "new.md", "# New");
        fs::write(
            library_root.join(".generation-broken/manifest.json"),
            b"not a manifest",
        )
        .expect("corrupt replacement manifest");

        assert!(reader.reload_current_bundle().is_err());
        assert_eq!(reader.entry_point, PathBuf::from("old.md"));
        assert_eq!(
            reader
                .reader
                .current_location()
                .expect("previous document location")
                .document
                .as_ref(),
            "old.md"
        );

        fs::remove_dir_all(library_root).expect("remove sync library fixture");
    }

    #[test]
    fn return_to_entry_point_clears_linked_document_navigation() {
        let root = fixture_root("# Entry\n\n[Chapter](chapter.md)");
        fs::write(root.join("chapter.md"), "# Chapter").expect("write linked document");
        let mut reader =
            T1Reader::open(fixture_config(&root), Viewport::new(240, 120)).expect("open fixture");
        reader
            .reader
            .follow_document("chapter.md")
            .expect("follow linked document");
        let event = reader
            .return_to_entry_point()
            .expect("return to entry point");
        assert!(matches!(event, ReaderEvent::Opened { .. }));
        assert_eq!(
            reader.reader.current_location().unwrap().document.as_ref(),
            "index.md"
        );
        fs::remove_dir_all(root).expect("remove reader fixture root");
    }
}
