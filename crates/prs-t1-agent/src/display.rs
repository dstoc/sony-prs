use crate::framebuffer::{DisplayCanvas, DisplayRegion, NativeDisplay, WaveformMode};
use embedded_graphics::mono_font::{
    ascii::{FONT_10X20, FONT_8X13, FONT_8X13_BOLD},
    MonoTextStyle,
};
use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};
use std::convert::Infallible;

const BLACK: u16 = 0x0000;
const WHITE: u16 = 0xffff;

pub const STATUS_BAR_HEIGHT: usize = 48;
pub const CONTENT_TOP: usize = 76;
pub const CONTENT_LINE_STEP: usize = 25;
pub const DETAILS_LINE_STEP: usize = 21;
pub const DETAILS_ACTION_MARGIN: usize = 24;
pub const DETAILS_REBOOT_TOP: usize = 616;
pub const DETAILS_POWER_OFF_TOP: usize = 674;
pub const DETAILS_BACK_TOP: usize = 732;
pub const DETAILS_ACTION_HEIGHT: usize = 48;
pub const SCREEN_WIDTH: usize = 600;

const STATUS_BAR_SIDE_MARGIN: usize = 16;
const STATUS_BAR_CLOCK_WIDTH: usize = 96;
const STATUS_ICON_GAP: usize = 4;
const STATUS_MODE_WIDTH: usize = 96;
const BATTERY_ICON: &[u8] = include_bytes!("../assets/battery-20x20.bin");
const WIFI_ICON: &[u8] = include_bytes!("../assets/wifi-20x20.bin");
const USB_ICON: &[u8] = include_bytes!("../assets/usb-20x20.bin");
const ADB_ICON: &[u8] = include_bytes!("../assets/adb-20x20.bin");
const CLOCK_ICON: &[u8] = include_bytes!("../assets/clock-20x20.bin");

pub fn draw_screen(
    display: &mut NativeDisplay,
    lines: &[String],
    refresh_region: DisplayRegion,
    waveform: WaveformMode,
    wait_for_completion: bool,
    force_refresh: bool,
) -> std::io::Result<()> {
    let frame = render_screen(lines, display.width() as usize, display.height() as usize);
    display.draw_frame_with_waveform(
        &frame,
        refresh_region,
        waveform,
        wait_for_completion,
        force_refresh,
    )
}

fn draw_screen_contents(canvas: &mut DisplayCanvas<'_>, lines: &[String]) {
    canvas.fill(WHITE);
    draw_status_bar(canvas, lines);

    let details = lines
        .get(1)
        .is_some_and(|line| line == "Details / Settings");
    let line_step = if details {
        DETAILS_LINE_STEP
    } else {
        CONTENT_LINE_STEP
    };
    for (index, line) in lines.iter().skip(1).enumerate() {
        let y = CONTENT_TOP.saturating_add(index.saturating_mul(line_step));
        if details {
            if index == 0 {
                draw_text_font(canvas, 24, y, line, &FONT_10X20, Rgb565::BLACK);
            } else if is_section_heading(line) {
                draw_section_heading(canvas, y, line);
            } else {
                draw_text(canvas, 24, y, line);
            }
        } else {
            if index == 0 {
                draw_text_centered_font(canvas, y, line, &FONT_10X20, Rgb565::BLACK);
            } else {
                draw_text_centered(canvas, y, line, Rgb565::BLACK);
            }
        }
    }

    if details {
        draw_details_actions(canvas);
    }
}

/// Render a logical screen into the format expected by the EPDC standby
/// framebuffer ioctl. This buffer has no virtual-screen padding or offsets.
pub fn standby_screen(lines: &[String], width: usize, height: usize) -> Vec<u8> {
    render_screen(lines, width, height)
}

fn render_screen(lines: &[String], width: usize, height: usize) -> Vec<u8> {
    let mut image = vec![0u8; width.saturating_mul(height).saturating_mul(2)];
    {
        let mut canvas =
            DisplayCanvas::new(&mut image, width, height, width.saturating_mul(2), 0, 0);
        draw_screen_contents(&mut canvas, lines);
    }
    image
}

pub(crate) fn draw_status_bar(canvas: &mut DisplayCanvas<'_>, lines: &[String]) {
    let width = canvas.width();
    canvas.fill_rect(0, 0, width, STATUS_BAR_HEIGHT, BLACK);

    let Some(line) = lines.first() else {
        return;
    };
    let fields = line.split('|').collect::<Vec<_>>();
    let side_margin = STATUS_BAR_SIDE_MARGIN.min(width / 2);
    let clock_width = STATUS_BAR_CLOCK_WIDTH.min(width.saturating_sub(side_margin * 2));
    let clock_left = width.saturating_sub(side_margin + clock_width);
    let mut left = side_margin;

    if let Some(value) = fields.first().copied() {
        left = left.saturating_add(draw_status_value(canvas, left, value, BATTERY_ICON));
        left = left.saturating_add(8);
    }
    for (index, sprite) in [(1, WIFI_ICON), (2, USB_ICON), (3, ADB_ICON)] {
        if let Some(value) = fields.get(index).copied() {
            if status_icon_is_on(value) {
                draw_icon_sprite(canvas, left, 14, sprite);
                left = left.saturating_add(20 + STATUS_ICON_GAP);
            }
        }
    }

    if let Some(value) = fields.get(4).copied() {
        if !value.is_empty() {
            let mode_left = clock_left.saturating_sub(STATUS_MODE_WIDTH + 16);
            draw_text_centered_in_rect_font(
                canvas,
                mode_left,
                0,
                STATUS_MODE_WIDTH,
                STATUS_BAR_HEIGHT,
                value,
                &FONT_8X13_BOLD,
                Rgb565::WHITE,
            );
        }
    }
    if let Some(value) = fields.get(5).copied() {
        draw_status_value(canvas, clock_left, value, CLOCK_ICON);
    }
}

/// Draw the T1-owned short-lived interaction message in the breathing room
/// between the status bar and the Markdown viewport.
pub(crate) fn draw_reader_feedback(canvas: &mut DisplayCanvas<'_>, text: &str) {
    if text.is_empty() {
        return;
    }
    draw_text_centered_in_rect(
        canvas,
        0,
        STATUS_BAR_HEIGHT,
        canvas.width(),
        CONTENT_TOP.saturating_sub(STATUS_BAR_HEIGHT),
        text,
        Rgb565::BLACK,
    );
}

fn draw_status_value(
    canvas: &mut DisplayCanvas<'_>,
    left: usize,
    value: &str,
    sprite: &[u8],
) -> usize {
    let icon_width = 20;
    let gap = 6;
    let style = MonoTextStyle::new(&FONT_8X13_BOLD, Rgb565::WHITE);
    let measured = Text::with_baseline(value, Point::zero(), style, Baseline::Top);
    let text_width = measured.bounding_box().size.width as usize;
    let text_y = (STATUS_BAR_HEIGHT.saturating_sub(13)) / 2;
    draw_icon_sprite(canvas, left, 14, sprite);
    draw_text_font(
        canvas,
        left.saturating_add(icon_width + gap),
        text_y,
        value,
        &FONT_8X13_BOLD,
        Rgb565::WHITE,
    );
    icon_width + gap + text_width
}

fn draw_icon_sprite(canvas: &mut DisplayCanvas<'_>, left: usize, top: usize, sprite: &[u8]) {
    for y in 0..20 {
        for x in 0..20 {
            let byte = sprite[y * 3 + x / 8];
            if byte & (0x80 >> (x % 8)) != 0 {
                canvas.set_pixel(left.saturating_add(x), top.saturating_add(y), WHITE);
            }
        }
    }
}

fn status_icon_is_on(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "on" | "up" | "running")
}

fn is_section_heading(line: &str) -> bool {
    matches!(
        line,
        "Power" | "Connectivity" | "System" | "Storage" | "Input"
    )
}

fn draw_section_heading(canvas: &mut DisplayCanvas<'_>, y: usize, text: &str) {
    draw_text_font(canvas, 24, y, text, &FONT_8X13_BOLD, Rgb565::BLACK);
    canvas.fill_rect(
        24,
        y.saturating_add(15),
        canvas.width().saturating_sub(48),
        1,
        BLACK,
    );
}

fn draw_details_actions(canvas: &mut DisplayCanvas<'_>) {
    let margin = DETAILS_ACTION_MARGIN;
    let width = canvas.width();
    let button_width = width.saturating_sub(margin * 2);
    for (top, label) in [
        (DETAILS_REBOOT_TOP, "Reboot"),
        (DETAILS_POWER_OFF_TOP, "Power off"),
        (DETAILS_BACK_TOP, "Back to reading"),
    ] {
        canvas.stroke_rect(margin, top, button_width, DETAILS_ACTION_HEIGHT, BLACK);
        draw_text_centered_in_rect(
            canvas,
            margin,
            top,
            button_width,
            DETAILS_ACTION_HEIGHT,
            label,
            Rgb565::BLACK,
        );
    }
}

fn draw_text(canvas: &mut DisplayCanvas<'_>, x: usize, y: usize, text: &str) {
    draw_text_font(canvas, x, y, text, &FONT_8X13, Rgb565::BLACK);
}

fn draw_text_font(
    canvas: &mut DisplayCanvas<'_>,
    x: usize,
    y: usize,
    text: &str,
    font: &'static embedded_graphics::mono_font::MonoFont<'static>,
    color: Rgb565,
) {
    let style = MonoTextStyle::new(font, color);
    Text::with_baseline(text, Point::new(x as i32, y as i32), style, Baseline::Top)
        .draw(canvas)
        .expect("RGB565 framebuffer drawing is infallible");
}

fn draw_text_centered(canvas: &mut DisplayCanvas<'_>, y: usize, text: &str, color: Rgb565) {
    draw_text_centered_font(canvas, y, text, &FONT_8X13, color);
}

fn draw_text_centered_font(
    canvas: &mut DisplayCanvas<'_>,
    y: usize,
    text: &str,
    font: &'static embedded_graphics::mono_font::MonoFont<'static>,
    color: Rgb565,
) {
    let style = MonoTextStyle::new(font, color);
    let measured = Text::with_baseline(text, Point::zero(), style, Baseline::Top);
    let text_width = measured.bounding_box().size.width as usize;
    let x = canvas.width().saturating_sub(text_width) / 2;
    measured
        .translate(Point::new(x as i32, y as i32))
        .draw(canvas)
        .expect("RGB565 framebuffer drawing is infallible");
}

fn draw_text_centered_in_rect(
    canvas: &mut DisplayCanvas<'_>,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
    text: &str,
    color: Rgb565,
) {
    draw_text_centered_in_rect_font(
        canvas,
        left,
        top,
        width,
        height,
        text,
        &FONT_8X13_BOLD,
        color,
    );
}

fn draw_text_centered_in_rect_font(
    canvas: &mut DisplayCanvas<'_>,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
    text: &str,
    font: &'static embedded_graphics::mono_font::MonoFont<'static>,
    color: Rgb565,
) {
    let style = MonoTextStyle::new(font, color);
    let measured = Text::with_baseline(text, Point::zero(), style, Baseline::Top);
    let bounds = measured.bounding_box();
    let x = left.saturating_add(width.saturating_sub(bounds.size.width as usize) / 2);
    let y = top.saturating_add(height.saturating_sub(bounds.size.height as usize) / 2);
    measured
        .translate(Point::new(x as i32, y as i32))
        .draw(canvas)
        .expect("RGB565 framebuffer drawing is infallible");
}

impl OriginDimensions for DisplayCanvas<'_> {
    fn size(&self) -> Size {
        Size::new(self.width() as u32, self.height() as u32)
    }
}

impl DrawTarget for DisplayCanvas<'_> {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x >= 0 && point.y >= 0 {
                self.set_pixel(point.x as usize, point.y as usize, color.into_storage());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::standby_screen;

    #[test]
    fn standby_screen_is_unpadded_rgb565() {
        let image = standby_screen(&[], 100, 100);

        assert_eq!(image.len(), 100 * 100 * 2);
        let body_offset = (70 * 100 + 1) * 2;
        assert_eq!(
            &image[body_offset..body_offset + 2],
            &0xffffu16.to_ne_bytes()
        );
        let header_offset = (1 * 100 + 1) * 2;
        assert_eq!(
            &image[header_offset..header_offset + 2],
            &0u16.to_ne_bytes()
        );
    }
}
