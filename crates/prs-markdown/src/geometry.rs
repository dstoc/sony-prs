//! Hardware-independent page geometry.
//!
//! All rectangles in the reader IR use the same convention as
//! `embedded-graphics`: the top-left point is inclusive and the size extends
//! to the right and down.  A page's `(0, 0)` is the top-left of its reader
//! viewport; applications can translate that page to a device-specific
//! framebuffer origin later.

use crate::style::ReaderStyle;
use embedded_graphics::geometry::Point;
use embedded_graphics::primitives::Rectangle;

/// The rectangle type shared by layout, pagination, and rendering.
pub type Rect = Rectangle;

/// A reader viewport expressed in page-space units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

/// The visible PRS-T1 framebuffer used by the host reader profile.
pub const T1_VIEWPORT: Viewport = Viewport::new(600, 800);

/// The logical landscape dimensions supported by the shared reader layout.
pub const T1_LANDSCAPE_VIEWPORT: Viewport = Viewport::new(800, 600);

/// The neutral font scale used by the compatibility reader constructors.
pub const DEFAULT_FONT_SCALE_PERCENT: u16 = 100;

/// The fixed page-space size of a rendered GFM task checkbox.
pub const TASK_CHECKBOX_SIZE: u32 = 12;

/// The gap between a task checkbox and the task text.
pub const TASK_CHECKBOX_GAP: u32 = 4;

impl Viewport {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Returns the page-space rectangle covered by this viewport.
    pub const fn bounds(self) -> Rect {
        Rect::new(
            Point::zero(),
            embedded_graphics::geometry::Size::new(self.width, self.height),
        )
    }

    pub fn contains(self, point: Point) -> bool {
        self.bounds().contains(point)
    }

    /// Clips a page-space rectangle to this viewport.
    pub fn clip(self, rectangle: Rect) -> Option<Rect> {
        intersection(rectangle, self.bounds())
    }
}

/// The part of a reader surface reserved for a progress indicator.
///
/// The reader does not draw the indicator. It reserves the area so that a
/// native shell, browser shell, or later settings surface can draw it without
/// changing document pagination.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProgressLineArea {
    pub reserved_height: u32,
}

impl ProgressLineArea {
    pub const fn new(reserved_height: u32) -> Self {
        Self { reserved_height }
    }
}

/// Shared presentation inputs for native and browser reader surfaces.
///
/// `display_viewport` is the logical surface supplied by the host. The
/// effective page viewport removes the reserved status-bar and progress-line
/// areas. `font_scale_percent` is fixed-point: `100` is the default size,
/// `125` is 125 percent, and values below 100 reduce the text size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReaderLayout {
    pub display_viewport: Viewport,
    pub status_bar_height: u32,
    pub progress_line: ProgressLineArea,
    pub font_scale_percent: u16,
}

impl ReaderLayout {
    pub const fn new(display_viewport: Viewport) -> Self {
        Self {
            display_viewport,
            status_bar_height: 0,
            progress_line: ProgressLineArea::new(0),
            font_scale_percent: DEFAULT_FONT_SCALE_PERCENT,
        }
    }

    /// Build a layout whose supplied viewport is already the page viewport.
    pub const fn content(viewport: Viewport) -> Self {
        Self::new(viewport)
    }

    pub const fn with_status_bar_height(mut self, status_bar_height: u32) -> Self {
        self.status_bar_height = status_bar_height;
        self
    }

    pub const fn with_progress_line_height(mut self, reserved_height: u32) -> Self {
        self.progress_line = ProgressLineArea::new(reserved_height);
        self
    }

    pub const fn with_font_scale_percent(mut self, font_scale_percent: u16) -> Self {
        self.font_scale_percent = font_scale_percent;
        self
    }

    /// Return the page-space viewport available to Markdown content.
    pub const fn effective_viewport(self) -> Viewport {
        Viewport::new(
            self.display_viewport.width,
            self.display_viewport
                .height
                .saturating_sub(self.status_bar_height)
                .saturating_sub(self.progress_line.reserved_height),
        )
    }

    /// Return the page-space y coordinate at which content is drawn.
    pub const fn content_top(self) -> u32 {
        self.status_bar_height
    }

    /// Return the reserved progress-line rectangle in display coordinates.
    pub const fn progress_line_bounds(self) -> Rect {
        Rect::new(
            Point::new(
                0,
                self.content_top() as i32 + self.effective_viewport().height as i32,
            ),
            embedded_graphics::geometry::Size::new(
                self.display_viewport.width,
                self.progress_line.reserved_height,
            ),
        )
    }

    pub const fn is_landscape(self) -> bool {
        self.display_viewport.width > self.display_viewport.height
    }

    /// Apply the layout's font scale to a caller-supplied reader style.
    pub fn effective_style(self, style: ReaderStyle) -> ReaderStyle {
        style.scaled_font(self.font_scale_percent)
    }
}

/// Translate a rectangle by a page-space offset without changing its size.
pub fn translate(rectangle: Rect, offset: Point) -> Rect {
    Rect::new(
        Point::new(
            rectangle.top_left.x.saturating_add(offset.x),
            rectangle.top_left.y.saturating_add(offset.y),
        ),
        rectangle.size,
    )
}

/// Return the non-empty intersection of two rectangles.
pub fn intersection(first: Rect, second: Rect) -> Option<Rect> {
    let result = first.intersection(&second);
    (!result.is_zero_sized()).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::geometry::Size;

    #[test]
    fn translation_preserves_size_and_moves_origin() {
        let original = Rect::new(Point::new(4, 8), Size::new(20, 10));

        assert_eq!(
            translate(original, Point::new(-3, 5)),
            Rect::new(Point::new(1, 13), Size::new(20, 10))
        );
    }

    #[test]
    fn viewport_clips_and_rejects_non_intersecting_rectangles() {
        let viewport = Viewport::new(20, 12);

        assert_eq!(
            viewport.clip(Rect::new(Point::new(-3, 8), Size::new(10, 10))),
            Some(Rect::new(Point::new(0, 8), Size::new(7, 4)))
        );
        assert_eq!(
            viewport.clip(Rect::new(Point::new(20, 0), Size::new(2, 2))),
            None
        );
    }

    #[test]
    fn reader_layout_reserves_status_and_progress_areas() {
        let layout = ReaderLayout::new(T1_VIEWPORT)
            .with_status_bar_height(76)
            .with_progress_line_height(16)
            .with_font_scale_percent(125);

        assert_eq!(layout.effective_viewport(), Viewport::new(600, 708));
        assert_eq!(layout.content_top(), 76);
        assert_eq!(
            layout.progress_line_bounds(),
            Rect::new(Point::new(0, 784), Size::new(600, 16))
        );
        assert!(!layout.is_landscape());
        assert_eq!(layout.font_scale_percent, 125);
    }

    #[test]
    fn reader_layout_supports_landscape_without_portrait_assumptions() {
        let layout = ReaderLayout::new(T1_LANDSCAPE_VIEWPORT)
            .with_status_bar_height(32)
            .with_progress_line_height(8);

        assert_eq!(layout.effective_viewport(), Viewport::new(800, 560));
        assert_eq!(
            layout.progress_line_bounds(),
            Rect::new(Point::new(0, 592), Size::new(800, 8))
        );
        assert!(layout.is_landscape());
    }
}
