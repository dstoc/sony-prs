//! Hardware-independent page geometry.
//!
//! All rectangles in the reader IR use the same convention as
//! `embedded-graphics`: the top-left point is inclusive and the size extends
//! to the right and down.  A page's `(0, 0)` is the top-left of its reader
//! viewport; applications can translate that page to a device-specific
//! framebuffer origin later.

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
}
