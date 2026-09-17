//! Generic rendering boundary for a paginated display list.
//!
//! [`EmbeddedGraphicsRenderer`] consumes only a [`PageLayout`], a caller-owned
//! [`TextEngine`], and an `embedded-graphics` draw target. It does not inspect
//! Markdown blocks or make layout decisions. Page coordinates are translated
//! to the requested target origin and clipped to the page viewport before any
//! pixels are submitted.

use crate::geometry::{translate, Rect};
use crate::image::RasterImage;
use crate::pagination::{DisplayCommand, PageLayout};
use crate::style::{BorderStyle, Color, TextStyle as ReaderTextStyle};
use crate::typography::{FontFace, GlyphBitmap, TextEngine, TextRun, TextStyle};
use embedded_graphics::draw_target::{DrawTarget, DrawTargetExt};
use embedded_graphics::geometry::{Point, Size};
use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::prelude::Pixel;
use embedded_graphics::primitives::Rectangle;

/// A renderer that draws page display lists using a caller-supplied typography
/// backend.
///
/// `Rgb888` is used as the renderer's intermediate color type. The
/// `embedded-graphics` color-conversion adapter then converts it to any target
/// color type for which `Rgb888: Into<T::Color>` is implemented. This covers
/// the standard RGB/grayscale targets and the T1's `Rgb565` drawing canvas
/// without coupling this crate to a device pixel format.
#[derive(Debug)]
pub struct EmbeddedGraphicsRenderer<E> {
    text_engine: E,
}

impl<E> EmbeddedGraphicsRenderer<E> {
    /// Create a renderer with the typography backend that should rasterize
    /// text commands.
    pub const fn new(text_engine: E) -> Self {
        Self { text_engine }
    }

    pub fn text_engine(&self) -> &E {
        &self.text_engine
    }

    pub fn text_engine_mut(&mut self) -> &mut E {
        &mut self.text_engine
    }
}

impl<E: Default> Default for EmbeddedGraphicsRenderer<E> {
    fn default() -> Self {
        Self::new(E::default())
    }
}

impl<E: TextEngine> EmbeddedGraphicsRenderer<E> {
    /// Render a page with its viewport origin at `(0, 0)`.
    pub fn render<T>(&mut self, page: &PageLayout, target: &mut T) -> Result<(), T::Error>
    where
        T: DrawTarget,
        Rgb888: Into<T::Color>,
    {
        self.render_at(page, target, Point::zero())
    }

    /// Render a page at an arbitrary target-space origin.
    ///
    /// The page's viewport is translated by `origin` and used as the outer
    /// clip. The target's own bounding box remains an additional clip supplied
    /// by `embedded-graphics`.
    pub fn render_at<T>(
        &mut self,
        page: &PageLayout,
        target: &mut T,
        origin: Point,
    ) -> Result<(), T::Error>
    where
        T: DrawTarget,
        Rgb888: Into<T::Color>,
    {
        let mut converted = target.color_converted::<Rgb888>();
        let viewport = translate(page.viewport.bounds(), origin);
        let mut clipped = converted.clipped(&viewport);

        for command in page.display_list() {
            draw_command(&mut clipped, command, origin, &mut self.text_engine)?;
        }

        Ok(())
    }

    /// Alias for [`Self::render_at`] with the origin before the target, which
    /// is convenient for callers that group geometry arguments together.
    pub fn render_with_origin<T>(
        &mut self,
        page: &PageLayout,
        origin: Point,
        target: &mut T,
    ) -> Result<(), T::Error>
    where
        T: DrawTarget,
        Rgb888: Into<T::Color>,
    {
        self.render_at(page, target, origin)
    }
}

fn draw_command<T, E>(
    target: &mut T,
    command: &DisplayCommand,
    origin: Point,
    text_engine: &mut E,
) -> Result<(), T::Error>
where
    T: DrawTarget<Color = Rgb888>,
    E: TextEngine,
{
    match command {
        DisplayCommand::Fill { bounds, style } => {
            fill_rect(target, translate(*bounds, origin), style.color)
        }
        DisplayCommand::Border { bounds, style } => {
            draw_border(target, translate(*bounds, origin), *style)
        }
        DisplayCommand::TaskCheckbox { bounds, checked } => {
            draw_task_checkbox(target, translate(*bounds, origin), *checked)
        }
        DisplayCommand::Rule { bounds, style } => {
            fill_rect(target, translate(*bounds, origin), style.color)
        }
        DisplayCommand::Text {
            bounds,
            text,
            style,
            background,
        } => draw_text(
            target,
            translate(*bounds, origin),
            text,
            style,
            *background,
            text_engine,
        ),
        DisplayCommand::ImagePlaceholder { bounds, .. } => {
            draw_image_placeholder(target, translate(*bounds, origin))
        }
        DisplayCommand::Image { bounds, image, .. } => {
            draw_image(target, translate(*bounds, origin), image)
        }
    }
}

fn fill_rect<T>(target: &mut T, rectangle: Rect, color: Color) -> Result<(), T::Error>
where
    T: DrawTarget<Color = Rgb888>,
{
    if rectangle.size.width == 0 || rectangle.size.height == 0 {
        return Ok(());
    }
    target.fill_solid(&rectangle, to_rgb888(color))
}

fn draw_border<T>(target: &mut T, rectangle: Rect, style: BorderStyle) -> Result<(), T::Error>
where
    T: DrawTarget<Color = Rgb888>,
{
    let width = rectangle.size.width;
    let height = rectangle.size.height;
    let thickness = style.width.min(width).min(height);
    if thickness == 0 {
        return Ok(());
    }

    // A border thicker than half of either dimension necessarily fills the
    // whole narrow dimension. This also keeps all strip arithmetic bounded.
    if thickness.saturating_mul(2) >= width || thickness.saturating_mul(2) >= height {
        return fill_rect(target, rectangle, style.color);
    }

    let color = to_rgb888(style.color);
    target.fill_solid(
        &Rectangle::new(rectangle.top_left, Size::new(width, thickness)),
        color,
    )?;
    target.fill_solid(
        &Rectangle::new(
            Point::new(
                rectangle.top_left.x,
                rectangle
                    .top_left
                    .y
                    .saturating_add((height - thickness) as i32),
            ),
            Size::new(width, thickness),
        ),
        color,
    )?;

    let interior_height = height - thickness.saturating_mul(2);
    target.fill_solid(
        &Rectangle::new(
            Point::new(
                rectangle.top_left.x,
                rectangle.top_left.y.saturating_add(thickness as i32),
            ),
            Size::new(thickness, interior_height),
        ),
        color,
    )?;
    target.fill_solid(
        &Rectangle::new(
            Point::new(
                rectangle
                    .top_left
                    .x
                    .saturating_add((width - thickness) as i32),
                rectangle.top_left.y.saturating_add(thickness as i32),
            ),
            Size::new(thickness, interior_height),
        ),
        color,
    )
}

fn draw_task_checkbox<T>(target: &mut T, rectangle: Rect, checked: bool) -> Result<(), T::Error>
where
    T: DrawTarget<Color = Rgb888>,
{
    if rectangle.size.width == 0 || rectangle.size.height == 0 {
        return Ok(());
    }

    if checked {
        fill_rect(target, rectangle, Color::BLACK)?;
        let left = rectangle.top_left.x;
        let top = rectangle.top_left.y;
        for step in 0..4 {
            fill_rect(
                target,
                Rect::new(
                    Point::new(left.saturating_add(2 + step), top.saturating_add(4 + step)),
                    Size::new(2, 2),
                ),
                Color::WHITE,
            )?;
        }
        for step in 0..6 {
            fill_rect(
                target,
                Rect::new(
                    Point::new(left.saturating_add(5 + step), top.saturating_add(7 - step)),
                    Size::new(2, 2),
                ),
                Color::WHITE,
            )?;
        }
        Ok(())
    } else {
        fill_rect(target, rectangle, Color::WHITE)?;
        draw_border(target, rectangle, BorderStyle::new(Color::BLACK, 2))
    }
}

fn draw_text<T, E>(
    target: &mut T,
    bounds: Rect,
    text: &str,
    style: &ReaderTextStyle,
    background: Color,
    text_engine: &mut E,
) -> Result<(), T::Error>
where
    T: DrawTarget<Color = Rgb888>,
    E: TextEngine,
{
    if bounds.size.width == 0 || bounds.size.height == 0 || text.is_empty() {
        return Ok(());
    }

    let typography_style = TextStyle::new(font_face(style), style.font_size.max(1));
    let run = TextRun::new(text, typography_style);
    let layout = text_engine.wrap(&[run], bounds.size.width.max(1));
    let mut text_target = target.clipped(&bounds);

    for glyph in &layout.glyphs {
        let bitmap = text_engine.rasterize_glyph(glyph);
        draw_glyph(
            &mut text_target,
            bounds.top_left.x.saturating_add(glyph.x),
            bounds.top_left.y.saturating_add(glyph.y),
            &bitmap,
            style.ink,
            background,
        )?;
    }

    if style.underline {
        draw_underline(target, bounds, style.ink)?;
    }

    Ok(())
}

fn draw_underline<T>(target: &mut T, bounds: Rect, ink: u8) -> Result<(), T::Error>
where
    T: DrawTarget<Color = Rgb888>,
{
    if bounds.size.width == 0 || bounds.size.height < 2 {
        return Ok(());
    }

    // Leave one clear pixel below the line box's text area. This keeps the
    // one-pixel decoration separate from descenders on the T1's display.
    let y = bounds
        .top_left
        .y
        .saturating_add(bounds.size.height.saturating_sub(2) as i32);
    fill_rect(
        target,
        Rect::new(
            Point::new(bounds.top_left.x, y),
            Size::new(bounds.size.width, 1),
        ),
        Color::rgb(ink, ink, ink),
    )
}

fn font_face(style: &ReaderTextStyle) -> FontFace {
    FontFace::from_flags(style.code, style.bold, style.italic)
}

fn draw_glyph<T>(
    target: &mut T,
    x: i32,
    y: i32,
    bitmap: &GlyphBitmap,
    ink: u8,
    background: Color,
) -> Result<(), T::Error>
where
    T: DrawTarget<Color = Rgb888>,
{
    let width = bitmap.width as usize;
    if width == 0 || bitmap.height == 0 {
        return Ok(());
    }

    let pixels = bitmap
        .alpha
        .chunks(width)
        .take(bitmap.height as usize)
        .enumerate()
        .flat_map(|(row, alphas)| {
            alphas
                .iter()
                .enumerate()
                .filter(|(_, alpha)| **alpha != 0)
                .map(move |(column, alpha)| {
                    Pixel(
                        Point::new(
                            x.saturating_add(column as i32),
                            y.saturating_add(row as i32),
                        ),
                        coverage_color(ink, *alpha, background),
                    )
                })
        });

    target.draw_iter(pixels)
}

fn draw_image_placeholder<T>(target: &mut T, rectangle: Rect) -> Result<(), T::Error>
where
    T: DrawTarget<Color = Rgb888>,
{
    if rectangle.size.width == 0 || rectangle.size.height == 0 {
        return Ok(());
    }

    // Until an image decoder supplies pixels, keep image fragments visible and
    // deterministic without consulting the resource provider or device APIs.
    fill_rect(target, rectangle, Color::rgb(232, 232, 232))?;
    draw_border(target, rectangle, BorderStyle::new(Color::BLACK, 1))
}

fn draw_image<T>(target: &mut T, rectangle: Rect, image: &RasterImage) -> Result<(), T::Error>
where
    T: DrawTarget<Color = Rgb888>,
{
    if rectangle.size.width == 0 || rectangle.size.height == 0 {
        return Ok(());
    }
    let mut image_target = target.clipped(&rectangle);
    let width = rectangle.size.width;
    let height = rectangle.size.height;
    let pixels = (0..height).flat_map(|y| {
        (0..width).map(move |x| {
            let source_x = (u64::from(x) * u64::from(image.width()) / u64::from(width)) as u32;
            let source_y = (u64::from(y) * u64::from(image.height()) / u64::from(height)) as u32;
            let gray = image.pixel(source_x, source_y).unwrap_or(u8::MAX);
            Pixel(
                Point::new(
                    rectangle.top_left.x.saturating_add(x as i32),
                    rectangle.top_left.y.saturating_add(y as i32),
                ),
                Rgb888::new(gray, gray, gray),
            )
        })
    });
    image_target.draw_iter(pixels)
}

fn coverage_color(ink: u8, alpha: u8, background: Color) -> Rgb888 {
    let background = to_rgb888(background);
    let coverage = u16::from(alpha);
    let composite = |background_channel: u8| {
        ((u16::from(ink) * coverage + u16::from(background_channel) * (255 - coverage) + 127) / 255)
            as u8
    };
    Rgb888::new(
        composite(background.r()),
        composite(background.g()),
        composite(background.b()),
    )
}

/// Convert a device-independent RGBA value into the opaque intermediate color
/// understood by `embedded-graphics` drawing primitives.
///
/// `DrawTarget` has no read/modify/write operation, so translucent fills are
/// composited against white at this boundary. Glyph coverage is composited
/// against the background carried by its display-list text command, producing
/// deterministic antialiased grayscale pixels on host and RGB565 targets.
fn to_rgb888(color: Color) -> Rgb888 {
    let alpha = u16::from(color.alpha);
    let composite =
        |channel: u8| ((u16::from(channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
    Rgb888::new(
        composite(color.red),
        composite(color.green),
        composite(color.blue),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pagination::DisplayList;
    use crate::typography::{LineMetrics, PositionedGlyph, TextLayout, TextMetrics, TextRun};
    use embedded_graphics::geometry::Size;
    use embedded_graphics::mock_display::MockDisplay;
    use embedded_graphics::pixelcolor::{Rgb565, RgbColor};

    #[derive(Debug)]
    struct TestTextEngine {
        rasterizations: usize,
        alpha: Vec<u8>,
    }

    impl Default for TestTextEngine {
        fn default() -> Self {
            Self {
                rasterizations: 0,
                alpha: vec![255; 4],
            }
        }
    }

    impl TextEngine for TestTextEngine {
        fn measure(&self, _run: &TextRun<'_>) -> TextMetrics {
            TextMetrics {
                advance_width: 2.0,
                width: 2,
                line: LineMetrics {
                    ascent: 2,
                    descent: 0,
                    line_gap: 0,
                    line_height: 2,
                    baseline: 2,
                },
            }
        }

        fn wrap(&self, runs: &[TextRun<'_>], _available_width: u32) -> TextLayout {
            let text = runs.iter().map(|run| run.text).collect::<String>();
            let glyphs = text
                .chars()
                .enumerate()
                .map(|(index, character)| PositionedGlyph {
                    character,
                    glyph_id: 0,
                    face: FontFace::Regular,
                    font_size: 12,
                    x: index as i32 * 2,
                    y: 0,
                    width: 2,
                    height: 2,
                    advance_width: 2.0,
                    byte_offset: index,
                    span_id: None,
                })
                .collect::<Vec<_>>();
            TextLayout {
                lines: vec![crate::typography::TextLine {
                    glyph_range: 0..glyphs.len(),
                    width: glyphs.len() as u32 * 2,
                    metrics: LineMetrics {
                        ascent: 2,
                        descent: 0,
                        line_gap: 0,
                        line_height: 2,
                        baseline: 2,
                    },
                }],
                glyphs,
            }
        }

        fn rasterize_glyph(&mut self, _glyph: &PositionedGlyph) -> GlyphBitmap {
            self.rasterizations += 1;
            GlyphBitmap {
                width: 2,
                height: 2,
                left: 0,
                top: -2,
                advance_width: 2.0,
                alpha: self.alpha.clone(),
            }
        }
    }

    fn synthetic_page() -> PageLayout {
        let mut page = PageLayout::new(1, crate::geometry::Viewport::new(12, 10));
        let content = Rect::new(Point::new(1, 1), Size::new(10, 8));
        page.push_command(DisplayCommand::Fill {
            bounds: content,
            style: crate::style::FillStyle::new(Color::WHITE),
        });
        page.push_command(DisplayCommand::Border {
            bounds: content,
            style: BorderStyle::new(Color::BLACK, 1),
        });
        page.push_command(DisplayCommand::Text {
            bounds: Rect::new(Point::new(3, 3), Size::new(4, 2)),
            text: "x".into(),
            style: ReaderTextStyle::new(12, 14),
            background: Color::WHITE,
        });
        page.push_command(DisplayCommand::Rule {
            bounds: Rect::new(Point::new(2, 7), Size::new(8, 1)),
            style: BorderStyle::new(Color::BLACK, 1),
        });
        page.push_command(DisplayCommand::ImagePlaceholder {
            bounds: Rect::new(Point::new(8, 3), Size::new(2, 3)),
            source: Some("diagram.png".into()),
            alt: "diagram".into(),
        });
        page
    }

    #[test]
    fn renders_synthetic_page_at_origin_with_all_primitives() {
        let mut renderer = EmbeddedGraphicsRenderer::new(TestTextEngine::default());
        let mut display = MockDisplay::<Rgb888>::new();
        display.set_allow_overdraw(true);

        renderer.render(&synthetic_page(), &mut display).unwrap();

        assert_eq!(display.get_pixel(Point::new(1, 1)), Some(Rgb888::BLACK));
        assert_eq!(display.get_pixel(Point::new(2, 2)), Some(Rgb888::WHITE));
        assert_eq!(display.get_pixel(Point::new(3, 3)), Some(Rgb888::BLACK));
        assert_eq!(display.get_pixel(Point::new(5, 3)), Some(Rgb888::WHITE));
        assert_eq!(display.get_pixel(Point::new(2, 7)), Some(Rgb888::BLACK));
        assert_eq!(display.get_pixel(Point::new(8, 3)), Some(Rgb888::BLACK));
        assert_eq!(display.get_pixel(Point::new(8, 4)), Some(Rgb888::BLACK));
        assert_eq!(display.get_pixel(Point::new(7, 4)), Some(Rgb888::WHITE));
        assert_eq!(renderer.text_engine().rasterizations, 1);
    }

    #[test]
    fn renders_unchecked_and_checked_task_controls_in_high_contrast() {
        let page = PageLayout {
            number: 1,
            viewport: crate::geometry::Viewport::new(32, 16),
            range: crate::pagination::DocumentRange::default(),
            commands: DisplayList::from([
                DisplayCommand::TaskCheckbox {
                    bounds: Rect::new(Point::new(1, 1), Size::new(12, 12)),
                    checked: false,
                },
                DisplayCommand::TaskCheckbox {
                    bounds: Rect::new(Point::new(18, 1), Size::new(12, 12)),
                    checked: true,
                },
            ]),
            hit_regions: Vec::new(),
        };
        let mut renderer = EmbeddedGraphicsRenderer::new(TestTextEngine::default());
        let mut display = MockDisplay::<Rgb888>::new();
        display.set_allow_overdraw(true);

        renderer.render(&page, &mut display).unwrap();

        assert_eq!(display.get_pixel(Point::new(1, 1)), Some(Rgb888::BLACK));
        assert_eq!(display.get_pixel(Point::new(4, 4)), Some(Rgb888::WHITE));
        assert_eq!(display.get_pixel(Point::new(18, 1)), Some(Rgb888::BLACK));
        assert_eq!(display.get_pixel(Point::new(20, 5)), Some(Rgb888::WHITE));
        assert_eq!(display.get_pixel(Point::new(24, 8)), Some(Rgb888::WHITE));
    }

    #[test]
    fn underlined_text_draws_one_ink_matched_pixel_below_glyphs() {
        let mut renderer = EmbeddedGraphicsRenderer::new(TestTextEngine::default());
        let mut display = MockDisplay::<Rgb888>::new();
        display.set_allow_overdraw(true);
        display.clear(Rgb888::WHITE).unwrap();
        let style = ReaderTextStyle {
            underline: true,
            ink: 96,
            ..ReaderTextStyle::new(12, 4)
        };
        let bounds = Rect::new(Point::new(1, 1), Size::new(4, 4));
        let page = PageLayout {
            number: 1,
            viewport: crate::geometry::Viewport::new(8, 8),
            range: crate::pagination::DocumentRange::default(),
            commands: DisplayList::from([DisplayCommand::Text {
                bounds,
                text: "x".into(),
                style,
                background: Color::WHITE,
            }]),
            hit_regions: Vec::new(),
        };

        renderer.render(&page, &mut display).unwrap();

        assert_eq!(
            display.get_pixel(Point::new(1, 3)),
            Some(Rgb888::new(96, 96, 96))
        );
        assert_eq!(display.get_pixel(Point::new(5, 3)), Some(Rgb888::WHITE));
        assert_eq!(display.get_pixel(Point::new(1, 4)), Some(Rgb888::WHITE));
    }

    #[test]
    fn translates_and_clips_page_to_target_viewport() {
        let page = PageLayout {
            number: 1,
            viewport: crate::geometry::Viewport::new(5, 4),
            range: crate::pagination::DocumentRange::default(),
            commands: DisplayList::from([
                DisplayCommand::Fill {
                    bounds: Rect::new(Point::new(-2, -1), Size::new(5, 4)),
                    style: crate::style::FillStyle::new(Color::BLACK),
                },
                DisplayCommand::Text {
                    bounds: Rect::new(Point::new(3, 2), Size::new(2, 2)),
                    text: "x".into(),
                    style: ReaderTextStyle::new(12, 14),
                    background: Color::BLACK,
                },
            ]),
            hit_regions: Vec::new(),
        };
        let mut renderer = EmbeddedGraphicsRenderer::new(TestTextEngine::default());
        let mut display = MockDisplay::<Rgb888>::new();

        renderer
            .render_at(&page, &mut display, Point::new(10, 11))
            .unwrap();

        assert_eq!(display.get_pixel(Point::new(10, 11)), Some(Rgb888::BLACK));
        assert_eq!(display.get_pixel(Point::new(12, 13)), Some(Rgb888::BLACK));
        assert_eq!(display.get_pixel(Point::new(13, 13)), Some(Rgb888::BLACK));
        assert_eq!(display.get_pixel(Point::new(13, 11)), None);
        assert_eq!(display.get_pixel(Point::new(10, 15)), None);
        assert_eq!(display.get_pixel(Point::new(9, 11)), None);
        assert_eq!(renderer.text_engine().rasterizations, 1);
    }

    #[test]
    fn converts_renderer_colors_for_rgb565_targets() {
        let page = PageLayout {
            number: 1,
            viewport: crate::geometry::Viewport::new(2, 2),
            range: crate::pagination::DocumentRange::default(),
            commands: DisplayList::from([DisplayCommand::Fill {
                bounds: Rect::new(Point::zero(), Size::new(2, 2)),
                style: crate::style::FillStyle::new(Color::BLACK),
            }]),
            hit_regions: Vec::new(),
        };
        let mut renderer = EmbeddedGraphicsRenderer::new(TestTextEngine::default());
        let mut display = MockDisplay::<Rgb565>::new();

        renderer.render(&page, &mut display).unwrap();

        assert_eq!(display.get_pixel(Point::zero()), Some(Rgb565::BLACK));
        assert_eq!(display.get_pixel(Point::new(1, 1)), Some(Rgb565::BLACK));
    }

    #[test]
    fn rgba_and_glyph_coverage_are_composited_deterministically() {
        assert_eq!(to_rgb888(Color::rgba(0, 0, 0, 0)), Rgb888::WHITE);
        assert_eq!(
            coverage_color(0, 128, Color::WHITE),
            Rgb888::new(127, 127, 127)
        );
        assert_eq!(
            coverage_color(160, 255, Color::rgb(240, 240, 240)),
            Rgb888::new(160, 160, 160)
        );
    }

    #[test]
    fn glyph_coverage_uses_the_text_surface_background() {
        fn render(background: Color) -> MockDisplay<Rgb888> {
            let mut page = PageLayout::new(1, crate::geometry::Viewport::new(2, 2));
            page.push_command(DisplayCommand::Fill {
                bounds: Rect::new(Point::zero(), Size::new(2, 2)),
                style: crate::style::FillStyle::new(background),
            });
            page.push_command(DisplayCommand::Text {
                bounds: Rect::new(Point::zero(), Size::new(2, 2)),
                text: "x".into(),
                style: ReaderTextStyle::new(12, 14),
                background,
            });
            let mut renderer = EmbeddedGraphicsRenderer::new(TestTextEngine {
                rasterizations: 0,
                alpha: vec![128, 255, 0, 255],
            });
            let mut display = MockDisplay::<Rgb888>::new();
            display.set_allow_overdraw(true);
            renderer.render(&page, &mut display).unwrap();
            display
        }

        let white = render(Color::WHITE);
        let gray = render(Color::rgb(200, 200, 200));

        assert_eq!(
            white.get_pixel(Point::new(0, 0)),
            Some(Rgb888::new(127, 127, 127))
        );
        assert_eq!(
            gray.get_pixel(Point::new(0, 0)),
            Some(Rgb888::new(100, 100, 100))
        );
        assert_eq!(white.get_pixel(Point::new(1, 0)), Some(Rgb888::BLACK));
        assert_eq!(gray.get_pixel(Point::new(1, 0)), Some(Rgb888::BLACK));
        assert_eq!(white.get_pixel(Point::new(0, 1)), Some(Rgb888::WHITE));
        assert_eq!(
            gray.get_pixel(Point::new(0, 1)),
            Some(Rgb888::new(200, 200, 200))
        );
    }

    #[test]
    fn code_emphasis_selects_the_matching_monospace_face() {
        let expected = [
            (false, false, FontFace::Monospace),
            (true, false, FontFace::MonospaceBold),
            (false, true, FontFace::MonospaceItalic),
            (true, true, FontFace::MonospaceBoldItalic),
        ];
        for (bold, italic, expected_face) in expected {
            let style = ReaderTextStyle {
                code: true,
                bold,
                italic,
                ..ReaderTextStyle::new(12, 14)
            };
            assert_eq!(font_face(&style), expected_face);
        }
    }

    #[test]
    fn bold_code_draws_each_glyph_once_without_offset_double_draw() {
        let mut display = MockDisplay::<Rgb888>::new();
        display.set_allow_overdraw(true);
        display.clear(Rgb888::WHITE).unwrap();
        let style = ReaderTextStyle {
            bold: true,
            code: true,
            ..ReaderTextStyle::new(12, 14)
        };
        let mut text_engine = TestTextEngine::default();

        draw_text(
            &mut display,
            Rect::new(Point::zero(), Size::new(4, 1)),
            "x",
            &style,
            Color::WHITE,
            &mut text_engine,
        )
        .unwrap();

        assert_eq!(display.get_pixel(Point::new(0, 0)), Some(Rgb888::BLACK));
        assert_eq!(display.get_pixel(Point::new(1, 0)), Some(Rgb888::BLACK));
        assert_eq!(display.get_pixel(Point::new(2, 0)), Some(Rgb888::WHITE));
    }
}
