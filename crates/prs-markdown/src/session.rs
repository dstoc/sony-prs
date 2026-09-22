//! Shared reader interaction and transient presentation state.
//!
//! [`ReaderController`] sits above [`crate::Reader`] and below platform
//! adapters. It owns only state that must follow a user interaction across
//! native and browser readers. The low-level reader remains responsible for
//! parsing, pagination, navigation, and history.

use crate::geometry::Rect;
use crate::layout::TextMeasurer;
use crate::parse::MarkdownParser;
use crate::reader::{Reader, ReaderError, ReaderEvent, ReaderRenderError};
use crate::render::EmbeddedGraphicsRenderer;
use crate::resources::ResourceProvider;
use crate::typography::TextEngine;
use embedded_graphics::draw_target::{DrawTarget, DrawTargetExt};
use embedded_graphics::geometry::{Point, Size};
use embedded_graphics::mono_font::{
    ascii::{FONT_8X13, FONT_8X13_BOLD},
    MonoTextStyle,
};
use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::prelude::{Drawable, Primitive};
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use qrcode::{types::Color, QrCode};
use std::error::Error;
use std::fmt;

pub const EXTERNAL_LINK_OVERLAY_WIDTH: u32 = 360;
pub const EXTERNAL_LINK_OVERLAY_HEIGHT: u32 = 214;
pub const EXTERNAL_LINK_OVERLAY_MARGIN: u32 = 12;
pub const EXTERNAL_LINK_QR_BOX_SIZE: u32 = 176;
pub const EXTERNAL_LINK_QR_QUIET_ZONE: u32 = 4;
pub const EXTERNAL_LINK_BOTTOM_MARGIN: u32 = 16;

/// Return the shared bottom-right damage and rendering region for an external
/// link overlay.
pub fn external_link_overlay_region(width: u32, height: u32) -> Rect {
    let overlay_width = EXTERNAL_LINK_OVERLAY_WIDTH.min(width);
    let overlay_height =
        EXTERNAL_LINK_OVERLAY_HEIGHT.min(height.saturating_sub(EXTERNAL_LINK_BOTTOM_MARGIN));
    let right_margin = EXTERNAL_LINK_OVERLAY_MARGIN.min(width.saturating_sub(overlay_width));
    let bottom = height
        .saturating_sub(EXTERNAL_LINK_BOTTOM_MARGIN)
        .min(height);
    let top = bottom.saturating_sub(overlay_height);
    Rectangle::new(
        Point::new(
            width
                .saturating_sub(right_margin)
                .saturating_sub(overlay_width) as i32,
            top as i32,
        ),
        Size::new(overlay_width, overlay_height),
    )
}

pub fn truncated_url_lines(url: &str, chars_per_line: usize, max_lines: usize) -> Vec<String> {
    if chars_per_line == 0 || max_lines == 0 {
        return Vec::new();
    }

    let mut remaining = url.chars().peekable();
    let mut lines = Vec::new();
    for line_index in 0..max_lines {
        let mut line = remaining.by_ref().take(chars_per_line).collect::<String>();
        let has_more = remaining.peek().is_some();
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

/// A small owned QR matrix used by the shared overlay and native authorization
/// screen. It contains no display or platform state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrMatrix {
    size: usize,
    modules: Vec<bool>,
}

impl QrMatrix {
    pub fn encode(value: &str) -> Result<Self, QrError> {
        if value.is_empty() {
            return Err(QrError::EmptyValue);
        }
        let code = QrCode::new(value.as_bytes()).map_err(|_| QrError::TooLarge)?;
        let size = code.width();
        let mut modules = Vec::with_capacity(size.saturating_mul(size));
        for y in 0..size {
            for x in 0..size {
                modules.push(code[(x, y)] == Color::Dark);
            }
        }
        Ok(Self { size, modules })
    }

    pub const fn size(&self) -> usize {
        self.size
    }

    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrError {
    EmptyValue,
    TooLarge,
}

impl fmt::Display for QrError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyValue => formatter.write_str("QR value must not be empty"),
            Self::TooLarge => formatter.write_str("value is too large for a QR code"),
        }
    }
}

impl Error for QrError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalLinkOverlay {
    url: String,
    qr: QrMatrix,
}

impl ExternalLinkOverlay {
    pub fn new(url: impl Into<String>) -> Result<Self, QrError> {
        let url = url.into();
        let qr = QrMatrix::encode(&url)?;
        Ok(Self { url, qr })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn qr(&self) -> &QrMatrix {
        &self.qr
    }

    /// Draw the overlay using the target's native color type. The shared
    /// renderer uses RGB888 internally, which keeps the geometry and pixels
    /// identical for RGB565 native output and RGBA browser output.
    pub fn render<T>(&self, target: &mut T) -> Result<(), T::Error>
    where
        T: DrawTarget,
        Rgb888: Into<T::Color>,
    {
        let size = target.bounding_box().size;
        let mut converted = target.color_converted::<Rgb888>();
        let region = external_link_overlay_region(size.width, size.height);
        let left = region.top_left.x.max(0) as u32;
        let top = region.top_left.y.max(0) as u32;
        let panel_width = region.size.width;
        let panel_height = region.size.height;
        if panel_width == 0 || panel_height == 0 {
            return Ok(());
        }

        let panel = Rectangle::new(Point::new(left as i32, top as i32), region.size);
        panel
            .into_styled(PrimitiveStyle::with_fill(Rgb888::WHITE))
            .draw(&mut converted)?;
        panel
            .into_styled(PrimitiveStyle::with_stroke(Rgb888::BLACK, 1))
            .draw(&mut converted)?;

        let qr_box_size = EXTERNAL_LINK_QR_BOX_SIZE
            .min(panel_width.saturating_sub(EXTERNAL_LINK_OVERLAY_MARGIN * 2))
            .min(panel_height.saturating_sub(EXTERNAL_LINK_OVERLAY_MARGIN * 2));
        let module_count = self
            .qr
            .size()
            .saturating_add((EXTERNAL_LINK_QR_QUIET_ZONE * 2) as usize);
        let scale = (qr_box_size as usize)
            .checked_div(module_count)
            .unwrap_or(0);
        if scale == 0 {
            return Ok(());
        }
        let qr_size = module_count.saturating_mul(scale);
        let qr_left = (left as usize)
            .saturating_add(panel_width as usize)
            .saturating_sub(EXTERNAL_LINK_OVERLAY_MARGIN as usize)
            .saturating_sub(qr_size);
        let qr_top =
            (top as usize).saturating_add((panel_height as usize).saturating_sub(qr_size) / 2);
        for y in 0..self.qr.size() {
            for x in 0..self.qr.size() {
                if self.qr.is_dark(x, y) {
                    converted.fill_solid(
                        &Rectangle::new(
                            Point::new(
                                qr_left.saturating_add(
                                    (x + EXTERNAL_LINK_QR_QUIET_ZONE as usize) * scale,
                                ) as i32,
                                qr_top.saturating_add(
                                    (y + EXTERNAL_LINK_QR_QUIET_ZONE as usize) * scale,
                                ) as i32,
                            ),
                            Size::new(scale as u32, scale as u32),
                        ),
                        Rgb888::BLACK,
                    )?;
                }
            }
        }

        let text_left = left.saturating_add(EXTERNAL_LINK_OVERLAY_MARGIN) as i32;
        let text_right = (qr_left as u32).saturating_sub(EXTERNAL_LINK_OVERLAY_MARGIN) as i32;
        let text_width = text_right.saturating_sub(text_left) as usize;
        let chars_per_line = text_width / FONT_8X13.character_size.width as usize;
        Text::with_baseline(
            "External link",
            Point::new(text_left, top.saturating_add(14) as i32),
            MonoTextStyle::new(&FONT_8X13_BOLD, Rgb888::BLACK),
            Baseline::Top,
        )
        .draw(&mut converted)?;
        Text::with_baseline(
            "URL:",
            Point::new(text_left, top.saturating_add(36) as i32),
            MonoTextStyle::new(&FONT_8X13, Rgb888::BLACK),
            Baseline::Top,
        )
        .draw(&mut converted)?;

        let line_height = FONT_8X13.character_size.height as usize + 2;
        let max_lines = (panel_height as usize)
            .saturating_sub(56)
            .checked_div(line_height)
            .unwrap_or(0);
        for (index, line) in truncated_url_lines(&self.url, chars_per_line, max_lines)
            .into_iter()
            .enumerate()
        {
            Text::with_baseline(
                &line,
                Point::new(
                    text_left,
                    top.saturating_add(54)
                        .saturating_add((index * line_height) as u32) as i32,
                ),
                MonoTextStyle::new(&FONT_8X13, Rgb888::BLACK),
                Baseline::Top,
            )
            .draw(&mut converted)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderControllerError {
    Reader(ReaderError),
    ExternalLinkQr(QrError),
}

impl fmt::Display for ReaderControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reader(error) => error.fmt(formatter),
            Self::ExternalLinkQr(error) => write!(formatter, "encode external URL as QR: {error}"),
        }
    }
}

impl Error for ReaderControllerError {}

impl From<ReaderError> for ReaderControllerError {
    fn from(error: ReaderError) -> Self {
        Self::Reader(error)
    }
}

/// Shared user-interaction controller for native and browser reader adapters.
pub struct ReaderController<
    P,
    M = crate::layout::ApproximateTextMeasurer,
    Parser = crate::parse::ComrakParser,
> where
    P: ResourceProvider,
    M: TextMeasurer + Clone,
    Parser: MarkdownParser,
{
    reader: Reader<P, M, Parser>,
    external_link_overlay: Option<ExternalLinkOverlay>,
}

impl<P, M, Parser> ReaderController<P, M, Parser>
where
    P: ResourceProvider,
    M: TextMeasurer + Clone,
    Parser: MarkdownParser,
{
    pub fn new(reader: Reader<P, M, Parser>) -> Self {
        Self {
            reader,
            external_link_overlay: None,
        }
    }

    pub fn reader(&self) -> &Reader<P, M, Parser> {
        &self.reader
    }

    pub fn external_link_overlay(&self) -> Option<&ExternalLinkOverlay> {
        self.external_link_overlay.as_ref()
    }

    pub fn clear_external_link_overlay(&mut self) -> bool {
        self.external_link_overlay.take().is_some()
    }

    pub fn render_external_link_overlay<T>(&self, target: &mut T) -> Result<(), T::Error>
    where
        T: DrawTarget,
        Rgb888: Into<T::Color>,
    {
        self.external_link_overlay
            .as_ref()
            .map_or(Ok(()), |overlay| overlay.render(target))
    }

    /// Render the current page and then compose the transient external-link
    /// overlay. The platform adapter supplies only the renderer, target, and
    /// target-space origin.
    pub fn render_current_page_with_overlay<E, T>(
        &mut self,
        renderer: &mut EmbeddedGraphicsRenderer<E>,
        target: &mut T,
        origin: Point,
    ) -> Result<(), ReaderRenderError<T::Error>>
    where
        E: TextEngine,
        T: DrawTarget,
        Rgb888: Into<T::Color>,
    {
        let page = self
            .reader
            .current_page()
            .ok_or(ReaderRenderError::Reader(ReaderError::NoDocumentOpen))?;
        renderer
            .render_at(page, target, origin)
            .map_err(ReaderRenderError::Target)?;
        self.render_external_link_overlay(target)
            .map_err(ReaderRenderError::Target)
    }

    pub fn open(&mut self) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| reader.open())
    }

    pub fn open_document(
        &mut self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| reader.open_document(path))
    }

    pub fn navigate_to_anchor(
        &mut self,
        anchor: &str,
    ) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| reader.navigate_to_anchor(anchor))
    }

    pub fn activate_at_or_page_turn(
        &mut self,
        point: Point,
    ) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| {
            let result = reader.activate_at(point)?;
            if !matches!(result, ReaderEvent::NoAction) {
                return Ok(result);
            }
            if point.x >= reader.viewport().width as i32 / 2 {
                reader.next_page_event()
            } else {
                reader.previous_page_event()
            }
        })
    }

    pub fn activate(
        &mut self,
        target: crate::navigation::NavigationTarget,
    ) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| reader.activate(target))
    }

    pub fn follow_reference(
        &mut self,
        reference: &str,
    ) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| reader.follow_reference(reference))
    }

    pub fn next_page(&mut self) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| reader.next_page_event())
    }

    pub fn previous_page(&mut self) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| reader.previous_page_event())
    }

    /// Rebuild pagination for a new viewport or reader style while retaining
    /// the current logical passage and history anchors.
    pub fn reflow(
        &mut self,
        viewport: crate::geometry::Viewport,
        style: crate::style::ReaderStyle,
    ) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| reader.reflow(viewport, style))
    }

    pub fn set_viewport(
        &mut self,
        viewport: crate::geometry::Viewport,
    ) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| reader.set_viewport(viewport))
    }

    pub fn set_style(
        &mut self,
        style: crate::style::ReaderStyle,
    ) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| reader.set_style(style))
    }

    pub fn back(&mut self) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| reader.back_event())
    }

    pub fn forward(&mut self) -> Result<ReaderEvent, ReaderControllerError> {
        self.dispatch(|reader| reader.forward_event())
    }

    fn dispatch<F>(&mut self, operation: F) -> Result<ReaderEvent, ReaderControllerError>
    where
        F: FnOnce(&mut Reader<P, M, Parser>) -> Result<ReaderEvent, ReaderError>,
    {
        // Every method here represents one user interaction. Clear first so
        // an external URL event from this same interaction can replace it.
        self.clear_external_link_overlay();
        let event = operation(&mut self.reader)?;
        if let ReaderEvent::ExternalUrl(url) = &event {
            self.external_link_overlay = Some(
                ExternalLinkOverlay::new(url.clone())
                    .map_err(ReaderControllerError::ExternalLinkQr)?,
            );
        }
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::ComrakParser;
    use crate::resources::BrowserResourceProvider;
    use crate::{Reader, ReaderStyle, T1_VIEWPORT};
    use embedded_graphics::geometry::Point;

    fn controller(
        source: &str,
    ) -> ReaderController<
        BrowserResourceProvider,
        crate::layout::ApproximateTextMeasurer,
        ComrakParser,
    > {
        let provider = BrowserResourceProvider::new(
            "README.md",
            vec![("README.md".into(), source.as_bytes().to_vec())],
        )
        .expect("create provider");
        let mut reader = Reader::new(provider, ReaderStyle::default(), T1_VIEWPORT);
        reader.open().expect("open reader");
        ReaderController::new(reader)
    }

    fn external_point(
        controller: &ReaderController<
            BrowserResourceProvider,
            crate::layout::ApproximateTextMeasurer,
            ComrakParser,
        >,
    ) -> Point {
        controller.reader().current_page().unwrap().hit_regions[0]
            .bounds
            .top_left
    }

    #[test]
    fn external_link_creates_overlay_and_next_inert_interaction_clears_it() {
        let mut controller =
            controller("[External](https://example.com/one)\n\n[Inert](#missing)\n");
        let point = external_point(&controller);
        assert_eq!(
            controller.activate_at_or_page_turn(point).unwrap(),
            ReaderEvent::ExternalUrl("https://example.com/one".into())
        );
        assert_eq!(
            controller.external_link_overlay().unwrap().url(),
            "https://example.com/one"
        );
        controller
            .activate_at_or_page_turn(Point::new(590, 790))
            .unwrap();
        assert!(controller.external_link_overlay().is_none());
    }

    #[test]
    fn navigation_clears_overlay_before_processing_and_external_links_replace_it() {
        let mut controller = controller(&format!(
            "[First](https://example.com/one)\n\n[Second](https://example.com/two)\n\n{}",
            "line\n".repeat(1000)
        ));
        let first = external_point(&controller);
        controller.activate_at_or_page_turn(first).unwrap();
        let second = controller.reader().current_page().unwrap().hit_regions[1]
            .bounds
            .top_left;
        controller.activate_at_or_page_turn(second).unwrap();
        assert_eq!(
            controller.external_link_overlay().unwrap().url(),
            "https://example.com/two"
        );
        assert!(matches!(
            controller.next_page().unwrap(),
            ReaderEvent::PageChanged { page: 1, .. }
        ));
        assert!(controller.external_link_overlay().is_none());
    }

    #[test]
    fn non_input_operations_do_not_dismiss_overlay() {
        let mut controller = controller("[External](https://example.com/one)\n");
        let point = external_point(&controller);
        controller.activate_at_or_page_turn(point).unwrap();
        let mut frame = embedded_graphics::mock_display::MockDisplay::<Rgb888>::new();
        frame.set_allow_overdraw(true);
        controller.render_external_link_overlay(&mut frame).unwrap();
        assert!(controller.external_link_overlay().is_some());
        assert!(controller.clear_external_link_overlay());
        assert!(!controller.clear_external_link_overlay());
    }
}
