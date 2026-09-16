//! Host-side helpers for exercising the complete Markdown reader pipeline.
//!
//! The host harness deliberately keeps the same hand-offs as the T1 reader:
//! parsing produces an owned [`Document`], layout produces positioned lines,
//! pagination produces [`PageLayout`] values, and rendering consumes those
//! page display lists. [`HostImage`] is only a draw target for inspecting a
//! rendered page; it does not contain a second preview implementation.

use crate::document::Document;
use crate::geometry::Viewport;
use crate::layout::{DocumentLayout, LayoutEngine, TextMeasurer};
use crate::navigation::NavigationTarget;
use crate::pagination::{DisplayCommand, PageLayout, Pagination, Paginator};
use crate::parse::{ComrakParser, ParseError};
use crate::render::EmbeddedGraphicsRenderer;
use crate::style::ReaderStyle;
use crate::typography::TextEngine;
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{Dimensions, Point, Size};
use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::prelude::Pixel;
use embedded_graphics::primitives::Rectangle;
use std::convert::Infallible;
use std::io::{self, Write};
use std::path::Path;

/// The parsed, laid-out, and paginated result of one host harness run.
#[derive(Clone, Debug)]
pub struct HostReader {
    document: Document,
    layout: DocumentLayout,
    pagination: Pagination,
}

impl HostReader {
    /// Run the production parser, approximate host measurer, and paginator.
    ///
    /// This is useful for structural tests because it does not require a
    /// system font. The CLI uses [`Self::from_source_with_measurer`] with a
    /// [`crate::typography::FontdueTextEngine`] for real host rendering.
    pub fn from_source(
        source: &str,
        style: ReaderStyle,
        viewport: Viewport,
    ) -> Result<Self, ParseError> {
        Self::from_source_with_measurer(
            source,
            style,
            viewport,
            crate::layout::ApproximateTextMeasurer,
        )
    }

    /// Run the production parser and use the supplied metric implementation
    /// for both width-sensitive layout and the resulting pagination.
    pub fn from_source_with_measurer<M: TextMeasurer>(
        source: &str,
        style: ReaderStyle,
        viewport: Viewport,
        measurer: M,
    ) -> Result<Self, ParseError> {
        let document = ComrakParser::new().parse(source)?;
        let layout = LayoutEngine::with_measurer(style, measurer).layout(&document, viewport);
        let pagination = Paginator::new(style).paginate(&layout);
        Ok(Self {
            document,
            layout,
            pagination,
        })
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn layout(&self) -> &DocumentLayout {
        &self.layout
    }

    pub fn pagination(&self) -> &Pagination {
        &self.pagination
    }

    pub fn page_count(&self) -> usize {
        self.pagination.page_count()
    }

    pub fn page(&self, index: usize) -> Option<&PageLayout> {
        self.pagination.page(index)
    }

    /// Return the text fragments visible on a page in display-list order.
    ///
    /// Display commands retain the fragment boundaries selected by layout,
    /// making this a useful structural assertion that is independent of font
    /// rasterization.
    pub fn visible_fragments(&self, index: usize) -> Option<Vec<&str>> {
        self.page(index).map(|page| {
            page.display_list()
                .iter()
                .filter_map(|command| match command {
                    DisplayCommand::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect()
        })
    }

    /// Resolve a page-space point against the page's semantic navigation
    /// regions without consulting rendered pixels.
    pub fn navigation_at(&self, page: usize, point: Point) -> Option<&NavigationTarget> {
        self.page(page)
            .and_then(|page| page.hit_test(point))
            .map(|region| &region.target)
    }
}

/// A host framebuffer that stores the renderer's output as 8-bit grayscale.
///
/// PGM was chosen as the interchange format because it is trivial to inspect,
/// has no metadata or compression surprises, and maps naturally to the
/// reader's e-ink-oriented grayscale output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl HostImage {
    pub fn new(width: u32, height: u32) -> Self {
        let pixel_count = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .expect("host image dimensions must fit in memory");
        Self {
            width,
            height,
            pixels: vec![u8::MAX; pixel_count],
        }
    }

    pub fn for_viewport(viewport: Viewport) -> Self {
        Self::new(viewport.width, viewport.height)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn pixel(&self, point: Point) -> Option<u8> {
        self.index(point).map(|index| self.pixels[index])
    }

    pub fn pgm_bytes(&self) -> Vec<u8> {
        let mut output = format!("P5\n{} {}\n255\n", self.width, self.height).into_bytes();
        output.extend_from_slice(&self.pixels);
        output
    }

    pub fn write_pgm<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(format!("P5\n{} {}\n255\n", self.width, self.height).as_bytes())?;
        writer.write_all(&self.pixels)
    }

    pub fn save_pgm(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let mut file = std::fs::File::create(path)?;
        self.write_pgm(&mut file)
    }

    fn index(&self, point: Point) -> Option<usize> {
        if point.x < 0
            || point.y < 0
            || point.x >= self.width as i32
            || point.y >= self.height as i32
        {
            return None;
        }
        let x = point.x as usize;
        let y = point.y as usize;
        y.checked_mul(self.width as usize)?.checked_add(x)
    }
}

impl Dimensions for HostImage {
    fn bounding_box(&self) -> Rectangle {
        Rectangle::new(Point::zero(), Size::new(self.width, self.height))
    }
}

impl DrawTarget for HostImage {
    type Color = Rgb888;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if let Some(index) = self.index(point) {
                // The renderer currently flattens colors to an opaque
                // Rgb888 target. Luminance keeps decorations and glyph
                // coverage useful in a grayscale inspection image.
                self.pixels[index] = ((u32::from(color.r()) * 299
                    + u32::from(color.g()) * 587
                    + u32::from(color.b()) * 114
                    + 500)
                    / 1000) as u8;
            }
        }
        Ok(())
    }
}

/// Render one production [`PageLayout`] into a host-inspectable PGM image.
pub fn render_page<E: TextEngine>(
    page: &PageLayout,
    renderer: &mut EmbeddedGraphicsRenderer<E>,
) -> HostImage {
    let mut image = HostImage::for_viewport(page.viewport());
    renderer
        .render(page, &mut image)
        .expect("HostImage drawing is infallible");
    image
}
