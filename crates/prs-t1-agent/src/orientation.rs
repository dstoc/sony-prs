//! The two physical reader orientations supported by the PRS-T1 shell.

use embedded_graphics::geometry::Point;
use prs_markdown::geometry::{Viewport, T1_LANDSCAPE_VIEWPORT, T1_VIEWPORT};

/// A reader orientation changes the physical framebuffer and the logical
/// reader surface together. Persistence is intentionally left to the settings
/// work that owns preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReaderOrientation {
    #[default]
    Portrait,
    Landscape,
}

impl ReaderOrientation {
    pub const ALL: [Self; 2] = [Self::Portrait, Self::Landscape];

    pub const fn viewport(self) -> Viewport {
        match self {
            Self::Portrait => T1_VIEWPORT,
            Self::Landscape => T1_LANDSCAPE_VIEWPORT,
        }
    }

    /// Linux fbdev's unrotated panel is 800x600. The portrait reader uses the
    /// existing 90-degree counterclockwise fbdev rotation.
    pub const fn framebuffer_rotation(self) -> u32 {
        match self {
            Self::Portrait => 3,
            Self::Landscape => 0,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Portrait => "Portrait",
            Self::Landscape => "Landscape",
        }
    }

    pub const fn toggle(self) -> Self {
        match self {
            Self::Portrait => Self::Landscape,
            Self::Landscape => Self::Portrait,
        }
    }

    pub const fn is_landscape(self) -> bool {
        matches!(self, Self::Landscape)
    }

    /// Map the legacy touch controller's native 800x600 axes into the active
    /// screen. The legacy touch device reports the panel axes, so portrait
    /// swaps them while landscape keeps them in their native order.
    pub const fn map_touch_point(self, raw_x: i32, raw_y: i32) -> Point {
        match self {
            Self::Portrait => Point::new(raw_y, raw_x),
            Self::Landscape => Point::new(raw_x, raw_y),
        }
    }

    /// Map the multitouch stream's portrait-sized screen coordinates into the
    /// active screen. The physical T1 reports this stream in the portrait
    /// coordinate order even when the framebuffer is configured for landscape.
    pub const fn map_multitouch_point(self, raw_x: i32, raw_y: i32) -> Point {
        match self {
            Self::Portrait => Point::new(raw_x, raw_y),
            Self::Landscape => Point::new(raw_y, raw_x),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_only_the_two_supported_physical_surfaces() {
        assert_eq!(
            ReaderOrientation::ALL,
            [ReaderOrientation::Portrait, ReaderOrientation::Landscape]
        );
        assert_eq!(
            ReaderOrientation::Portrait.viewport(),
            Viewport::new(600, 800)
        );
        assert_eq!(
            ReaderOrientation::Landscape.viewport(),
            Viewport::new(800, 600)
        );
    }

    #[test]
    fn maps_touch_axes_with_the_same_rotation_as_the_framebuffer() {
        assert_eq!(
            ReaderOrientation::Portrait.map_touch_point(770, 300),
            Point::new(300, 770)
        );
        assert_eq!(
            ReaderOrientation::Landscape.map_touch_point(770, 300),
            Point::new(770, 300)
        );
    }

    #[test]
    fn maps_multitouch_screen_coordinates_into_landscape_axes() {
        assert_eq!(
            ReaderOrientation::Portrait.map_multitouch_point(73, 771),
            Point::new(73, 771)
        );
        assert_eq!(
            ReaderOrientation::Landscape.map_multitouch_point(73, 771),
            Point::new(771, 73)
        );
    }
}
