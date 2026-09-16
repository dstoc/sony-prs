//! Pagination and semantic display-list construction.

use crate::layout::{DocumentLayout, LayoutBlockKind, LayoutLine, Viewport};
use crate::navigation::NavigationTarget;
use crate::style::{ReaderStyle, TextStyle};
use embedded_graphics::geometry::Point;
use embedded_graphics::primitives::Rectangle;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageLayout {
    pub number: usize,
    pub viewport: Viewport,
    pub commands: Vec<DisplayCommand>,
    pub hit_regions: Vec<HitRegion>,
}

impl PageLayout {
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn hit_test(&self, point: Point) -> Option<&HitRegion> {
        self.hit_regions.iter().find(|region| {
            point.x >= region.bounds.top_left.x
                && point.y >= region.bounds.top_left.y
                && point.x < region.bounds.top_left.x + region.bounds.size.width as i32
                && point.y < region.bounds.top_left.y + region.bounds.size.height as i32
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayCommand {
    Text {
        bounds: Rectangle,
        text: String,
        style: TextStyle,
    },
    Rule {
        bounds: Rectangle,
    },
    ImagePlaceholder {
        bounds: Rectangle,
        alt: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitRegion {
    pub bounds: Rectangle,
    pub target: NavigationTarget,
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
        let mut page = PageLayout {
            number: 1,
            viewport: layout.viewport,
            commands: Vec::new(),
            hit_regions: Vec::new(),
        };
        let mut page_origin = self.style.page_padding.top as i32;
        let page_height = layout
            .viewport
            .height
            .saturating_sub(
                self.style
                    .page_padding
                    .top
                    .saturating_add(self.style.page_padding.bottom),
            )
            .max(1) as i32;

        for block in layout.blocks() {
            for line in &block.lines {
                if line.bounds.top_left.y + line.bounds.size.height as i32
                    > page_origin + page_height
                    && !page.commands.is_empty()
                {
                    pages.push(page);
                    page = PageLayout {
                        number: pages.len() + 1,
                        viewport: layout.viewport,
                        commands: Vec::new(),
                        hit_regions: Vec::new(),
                    };
                    page_origin = line.bounds.top_left.y - self.style.page_padding.top as i32;
                }
                add_line(&mut page, line, page_origin, block.kind);
            }
        }

        if !page.commands.is_empty() || pages.is_empty() {
            pages.push(page);
        }
        pages
    }
}

fn add_line(page: &mut PageLayout, line: &LayoutLine, page_origin: i32, kind: LayoutBlockKind) {
    for fragment in &line.fragments {
        let bounds = translate(fragment.bounds, page_origin);
        page.commands.push(DisplayCommand::Text {
            bounds,
            text: fragment.text.clone(),
            style: fragment.style,
        });
        if let Some(target) = &fragment.link {
            page.hit_regions.push(HitRegion {
                bounds,
                target: target.clone(),
            });
        }
    }
    if line.fragments.is_empty() {
        page.commands.push(match kind {
            LayoutBlockKind::Rule => DisplayCommand::Rule {
                bounds: translate(line.bounds, page_origin),
            },
            LayoutBlockKind::Image => DisplayCommand::ImagePlaceholder {
                bounds: translate(line.bounds, page_origin),
                alt: String::new(),
            },
            _ => DisplayCommand::Rule {
                bounds: translate(line.bounds, page_origin),
            },
        });
    }
}

fn translate(rectangle: Rectangle, page_origin: i32) -> Rectangle {
    Rectangle::new(
        Point::new(rectangle.top_left.x, rectangle.top_left.y - page_origin),
        rectangle.size,
    )
}
