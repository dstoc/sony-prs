use crate::framebuffer::{DisplayCanvas, NativeDisplay};

const BLACK: u16 = 0x0000;
const WHITE: u16 = 0xffff;
const SCALE: usize = 3;
const CELL_WIDTH: usize = 6 * SCALE;

pub fn draw_screen(display: &mut NativeDisplay, lines: &[String]) -> std::io::Result<()> {
    display.draw(|canvas| {
        draw_pattern(canvas);
        let positions = [
            (24, 20),
            (24, 50),
            (28, 93),
            (28, 121),
            (28, 149),
            (28, 177),
            (28, 211),
            (28, 239),
            (28, 267),
            (28, 295),
            (28, 359),
            (28, 387),
            (28, 415),
            (28, 443),
            (28, 471),
            (28, 507),
            (28, 535),
            (28, 563),
            (28, 591),
        ];
        for (line, &(x, y)) in lines.iter().zip(positions.iter()) {
            draw_text(canvas, x, y, line);
        }
    })
}

/// Render a logical screen into the format expected by the EPDC standby
/// framebuffer ioctl. This buffer has no virtual-screen padding or offsets.
pub fn standby_screen(lines: &[String], width: usize, height: usize) -> Vec<u8> {
    let mut image = vec![0u8; width.saturating_mul(height).saturating_mul(2)];
    {
        let mut canvas =
            DisplayCanvas::new(&mut image, width, height, width.saturating_mul(2), 0, 0);
        draw_pattern(&mut canvas);
        let positions = [
            (24, 20),
            (24, 50),
            (28, 93),
            (28, 121),
            (28, 149),
            (28, 177),
            (28, 211),
            (28, 239),
            (28, 267),
            (28, 295),
            (28, 359),
            (28, 387),
            (28, 415),
            (28, 443),
            (28, 471),
            (28, 507),
            (28, 535),
            (28, 563),
            (28, 591),
        ];
        for (line, &(x, y)) in lines.iter().zip(positions.iter()) {
            draw_text(&mut canvas, x, y, line);
        }
    }
    image
}

fn draw_pattern(canvas: &mut DisplayCanvas<'_>) {
    canvas.fill(WHITE);
    let width = canvas.width();
    let height = canvas.height();
    canvas.stroke_rect(
        4,
        4,
        width.saturating_sub(8),
        height.saturating_sub(8),
        BLACK,
    );
    canvas.stroke_rect(16, 80, width.saturating_sub(32), 100, BLACK);
    canvas.stroke_rect(16, 198, width.saturating_sub(32), 132, BLACK);
    canvas.stroke_rect(16, 346, width.saturating_sub(32), 132, BLACK);
    canvas.stroke_rect(16, 494, width.saturating_sub(32), 120, BLACK);

    for (index, x) in (20..width.saturating_sub(20)).step_by(40).enumerate() {
        if index % 2 == 0 {
            canvas.fill_rect(x, height.saturating_sub(42), 20, 24, BLACK);
        }
    }
    canvas.hline(
        20,
        height.saturating_sub(12),
        width.saturating_sub(40),
        BLACK,
    );
    draw_target(canvas, width.saturating_sub(44), 92);
    draw_target(canvas, 28, height.saturating_sub(60));
    draw_target(canvas, width.saturating_sub(44), height.saturating_sub(60));
}

fn draw_target(canvas: &mut DisplayCanvas<'_>, x: usize, y: usize) {
    canvas.stroke_rect(x, y, 16, 16, BLACK);
    canvas.hline(x.saturating_sub(6), y + 8, 28, BLACK);
    canvas.vline(x + 8, y.saturating_sub(6), 28, BLACK);
}

fn draw_text(canvas: &mut DisplayCanvas<'_>, x: usize, y: usize, text: &str) {
    for (index, character) in text.chars().enumerate() {
        draw_glyph(canvas, x + index * CELL_WIDTH, y, character);
    }
}

fn draw_glyph(canvas: &mut DisplayCanvas<'_>, x: usize, y: usize, character: char) {
    let glyph = glyph(character);
    for (row, bits) in glyph.iter().enumerate() {
        for column in 0..5 {
            if bits & (1 << (4 - column)) != 0 {
                canvas.fill_rect(x + column * SCALE, y + row * SCALE, SCALE, SCALE, BLACK);
            }
        }
    }
}

fn glyph(character: char) -> [u8; 7] {
    match character {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        ':' => [0, 0b00100, 0, 0, 0b00100, 0, 0],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '/' => [0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0, 0],
        '.' => [0, 0, 0, 0, 0, 0b00110, 0b00110],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        _ => [0; 7],
    }
}

#[cfg(test)]
mod tests {
    use super::standby_screen;

    #[test]
    fn standby_screen_is_unpadded_rgb565() {
        let image = standby_screen(&[], 40, 40);

        assert_eq!(image.len(), 40 * 40 * 2);
        let interior_offset = (1 * 40 + 1) * 2;
        assert_eq!(
            &image[interior_offset..interior_offset + 2],
            &0xffffu16.to_ne_bytes()
        );
        let border_offset = (4 * 40 + 4) * 2;
        assert_eq!(
            &image[border_offset..border_offset + 2],
            &0u16.to_ne_bytes()
        );
    }
}
