//! Pagination and semantic display-list construction.
//!
//! A [`PageLayout`] is the handoff from layout to rendering. Its commands
//! are ordered back-to-front, use page-space coordinates, and contain no
//! Markdown or device concepts. The same page can consequently be rendered
//! by a framebuffer adapter, a host test target, or another renderer.

use crate::geometry::{translate, Rect, Viewport};
use crate::layout::{DocumentLayout, LayoutBlockKind, LayoutLine};
use crate::navigation::NavigationTarget;
use crate::style::{BorderStyle, Color, FillStyle, ReaderStyle, TextStyle};
use embedded_graphics::geometry::Point;

/// An ordered display list. Commands later in the list are drawn on top of
/// earlier commands.
pub type DisplayList = Vec<DisplayCommand>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageLayout {
    pub number: usize,
    pub viewport: Viewport,
    /// Commands are in back-to-front order and are expressed in page space.
    pub commands: DisplayList,
    /// Each entry is one page-space rectangle. Multiple entries may have the
    /// same target when a linked span wraps across lines.
    pub hit_regions: Vec<HitRegion>,
}

impl PageLayout {
    pub fn new(number: usize, viewport: Viewport) -> Self {
        Self {
            number,
            viewport,
            commands: DisplayList::new(),
            hit_regions: Vec::new(),
        }
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn display_list(&self) -> &DisplayList {
        &self.commands
    }

    /// Add a command after clipping its bounds to the page viewport.
    ///
    /// Clipping here gives renderers a useful invariant while still allowing
    /// them to clip again when the page is translated to a larger target.
    pub fn push_command(&mut self, command: DisplayCommand) {
        let Some(bounds) = self.viewport.clip(command.bounds()) else {
            return;
        };
        self.commands.push(command.with_bounds(bounds));
    }

    /// Add one semantic hit rectangle, clipped to page space.
    pub fn add_hit_region(&mut self, region: HitRegion) {
        let Some(bounds) = self.viewport.clip(region.bounds) else {
            return;
        };
        self.hit_regions.push(HitRegion { bounds, ..region });
    }

    /// Find the topmost semantic region containing `point`.
    ///
    /// Hit regions follow display-list ordering: later regions are topmost,
    /// so reverse iteration makes overlap behavior deterministic.
    pub fn hit_test(&self, point: Point) -> Option<&HitRegion> {
        if !self.viewport.contains(point) {
            return None;
        }
        self.hit_regions
            .iter()
            .rev()
            .find(|region| region.bounds.contains(point))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayCommand {
    /// A positioned text run. The renderer chooses how to rasterize it.
    Text {
        bounds: Rect,
        text: String,
        style: TextStyle,
    },
    /// A solid background or other rectangular fill.
    Fill { bounds: Rect, style: FillStyle },
    /// A rectangular border; the interior is left unchanged.
    Border { bounds: Rect, style: BorderStyle },
    /// A horizontal or vertical rule represented as a stroked rectangle.
    Rule { bounds: Rect, style: BorderStyle },
    /// A future image renderer can resolve `source`; until then the bounds and
    /// alt text are enough for a deterministic placeholder renderer.
    ImagePlaceholder {
        bounds: Rect,
        source: Option<String>,
        alt: String,
    },
}

impl DisplayCommand {
    pub fn bounds(&self) -> Rect {
        match self {
            Self::Text { bounds, .. }
            | Self::Fill { bounds, .. }
            | Self::Border { bounds, .. }
            | Self::Rule { bounds, .. }
            | Self::ImagePlaceholder { bounds, .. } => *bounds,
        }
    }

    fn with_bounds(self, bounds: Rect) -> Self {
        match self {
            Self::Text { text, style, .. } => Self::Text {
                bounds,
                text,
                style,
            },
            Self::Fill { style, .. } => Self::Fill { bounds, style },
            Self::Border { style, .. } => Self::Border { bounds, style },
            Self::Rule { style, .. } => Self::Rule { bounds, style },
            Self::ImagePlaceholder { source, alt, .. } => Self::ImagePlaceholder {
                bounds,
                source,
                alt,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitRegion {
    pub bounds: Rect,
    pub target: NavigationTarget,
}

impl HitRegion {
    pub fn new(bounds: Rect, target: NavigationTarget) -> Self {
        Self { bounds, target }
    }
}

pub struct Paginator {
    pub style: ReaderStyle,
}

impl Default for Paginator {
    fn default() -> Self {
        Self::new(ReaderStyle::default())
    }
}

impl Paginator {
    pub fn new(style: ReaderStyle) -> Self {
        Self { style }
    }

    pub fn paginate(&self, layout: &DocumentLayout) -> Vec<PageLayout> {
        let mut pages = Vec::new();
        let mut page_origin_y = 0;
        let mut page = PageLayout::new(1, layout.viewport);
        let page_height = layout.viewport.height.max(1) as i32;

        for block in layout.blocks() {
            for line in &block.lines {
                if line_bottom(line.bounds) > page_origin_y + page_height
                    && !page.commands.is_empty()
                {
                    pages.push(page);
                    page_origin_y = line
                        .bounds
                        .top_left
                        .y
                        .saturating_sub(self.style.page_padding.top as i32);
                    page = PageLayout::new(pages.len() + 1, layout.viewport);
                }
                add_line(
                    &mut page,
                    line,
                    Point::new(0, page_origin_y.saturating_neg()),
                    block.kind,
                );
            }
        }

        if !page.commands.is_empty() || pages.is_empty() {
            pages.push(page);
        }
        pages
    }
}

fn add_line(page: &mut PageLayout, line: &LayoutLine, page_offset: Point, kind: LayoutBlockKind) {
    for fragment in &line.fragments {
        let bounds = translate(fragment.bounds, page_offset);
        page.push_command(DisplayCommand::Text {
            bounds,
            text: fragment.text.clone(),
            style: fragment.style,
        });
        if let Some(target) = &fragment.link {
            page.add_hit_region(HitRegion::new(bounds, target.clone()));
        }
    }
    if line.fragments.is_empty() {
        let bounds = translate(line.bounds, page_offset);
        match kind {
            LayoutBlockKind::Rule => page.push_command(DisplayCommand::Rule {
                bounds,
                style: BorderStyle::new(Color::BLACK, 1),
            }),
            LayoutBlockKind::Image => page.push_command(DisplayCommand::ImagePlaceholder {
                bounds,
                source: None,
                alt: String::new(),
            }),
            _ => page.push_command(DisplayCommand::Rule {
                bounds,
                style: BorderStyle::new(Color::BLACK, 1),
            }),
        }
    }
}

fn line_bottom(rectangle: Rect) -> i32 {
    rectangle
        .top_left
        .y
        .saturating_add(rectangle.size.height.min(i32::MAX as u32) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Viewport;
    use crate::layout::{LayoutBlock, LayoutBlockKind, LayoutLine};
    use embedded_graphics::geometry::Size;

    fn page() -> PageLayout {
        PageLayout::new(1, Viewport::new(100, 80))
    }

    #[test]
    fn commands_are_translated_and_clipped_to_page_space() {
        let mut page = page();
        page.push_command(DisplayCommand::Fill {
            bounds: Rect::new(Point::new(-4, 6), Size::new(12, 10)),
            style: FillStyle::new(Color::WHITE),
        });

        assert_eq!(
            page.display_list()[0].bounds(),
            Rect::new(Point::new(0, 6), Size::new(8, 10))
        );
        assert_eq!(
            translate(
                Rect::new(Point::new(8, 40), Size::new(10, 10)),
                Point::new(3, -20)
            ),
            Rect::new(Point::new(11, 20), Size::new(10, 10))
        );
    }

    #[test]
    fn one_target_can_have_multiple_hit_rectangles() {
        let mut page = page();
        let target = NavigationTarget::from_destination("chapter.md#intro");
        page.add_hit_region(HitRegion::new(
            Rect::new(Point::new(5, 5), Size::new(30, 10)),
            target.clone(),
        ));
        page.add_hit_region(HitRegion::new(
            Rect::new(Point::new(5, 25), Size::new(25, 10)),
            target.clone(),
        ));

        assert_eq!(page.hit_regions.len(), 2);
        assert_eq!(page.hit_test(Point::new(10, 28)).unwrap().target, target);
    }

    #[test]
    fn overlapping_regions_use_the_last_added_region() {
        let mut page = page();
        page.add_hit_region(HitRegion::new(
            Rect::new(Point::new(10, 10), Size::new(30, 30)),
            NavigationTarget::Anchor("underlay".into()),
        ));
        page.add_hit_region(HitRegion::new(
            Rect::new(Point::new(20, 20), Size::new(30, 30)),
            NavigationTarget::Anchor("overlay".into()),
        ));

        assert_eq!(
            page.hit_test(Point::new(25, 25)).unwrap().target,
            NavigationTarget::Anchor("overlay".into())
        );
        assert!(page.hit_test(Point::new(90, 70)).is_none());
    }

    #[test]
    fn synthetic_page_contains_all_supported_primitives() {
        let mut page = page();
        let bounds = Rect::new(Point::new(4, 4), Size::new(80, 50));
        page.push_command(DisplayCommand::Fill {
            bounds,
            style: FillStyle::new(Color::rgba(245, 245, 245, 255)),
        });
        page.push_command(DisplayCommand::Border {
            bounds,
            style: BorderStyle::new(Color::BLACK, 2),
        });
        page.push_command(DisplayCommand::Text {
            bounds: Rect::new(Point::new(8, 8), Size::new(72, 12)),
            text: "A synthetic page".into(),
            style: TextStyle::new(12, 16),
        });
        page.push_command(DisplayCommand::Rule {
            bounds: Rect::new(Point::new(8, 26), Size::new(72, 2)),
            style: BorderStyle::new(Color::BLACK, 1),
        });
        page.push_command(DisplayCommand::ImagePlaceholder {
            bounds: Rect::new(Point::new(8, 32), Size::new(40, 18)),
            source: Some("images/diagram.png".into()),
            alt: "diagram".into(),
        });

        assert_eq!(page.display_list().len(), 5);
        assert_eq!(page.display_list()[0].bounds(), bounds);
        assert_eq!(page.display_list()[4].bounds().size, Size::new(40, 18));
    }

    #[test]
    fn paginator_translates_each_page_to_viewport_coordinates() {
        let viewport = Viewport::new(100, 30);
        let style = ReaderStyle {
            page_padding: crate::style::Insets {
                top: 4,
                right: 4,
                bottom: 4,
                left: 4,
            },
            ..ReaderStyle::default()
        };
        let layout = DocumentLayout {
            viewport,
            blocks: vec![
                LayoutBlock {
                    kind: LayoutBlockKind::Paragraph,
                    bounds: Rect::new(Point::new(4, 4), Size::new(40, 10)),
                    lines: vec![LayoutLine {
                        bounds: Rect::new(Point::new(4, 4), Size::new(40, 10)),
                        fragments: vec![crate::layout::LayoutFragment {
                            text: "first".into(),
                            bounds: Rect::new(Point::new(4, 4), Size::new(25, 10)),
                            style: style.body,
                            link: None,
                        }],
                    }],
                    anchor: None,
                },
                LayoutBlock {
                    kind: LayoutBlockKind::Paragraph,
                    bounds: Rect::new(Point::new(4, 34), Size::new(40, 10)),
                    lines: vec![LayoutLine {
                        bounds: Rect::new(Point::new(4, 34), Size::new(40, 10)),
                        fragments: vec![crate::layout::LayoutFragment {
                            text: "second".into(),
                            bounds: Rect::new(Point::new(4, 34), Size::new(30, 10)),
                            style: style.body,
                            link: None,
                        }],
                    }],
                    anchor: None,
                },
            ],
        };

        let pages = Paginator::new(style).paginate(&layout);

        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].display_list()[0].bounds().top_left.y, 4);
        assert_eq!(pages[1].display_list()[0].bounds().top_left.y, 4);
    }
}
