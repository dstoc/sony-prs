use crate::framebuffer::DisplayRegion;
use std::io;

/// Optional expansion and alignment applied after a changed-pixel scan.
///
/// The T1 alignment requirements are not established yet, so the default is
/// an exact bounding box with no padding. Keeping these knobs here lets us
/// tune them from device measurements without changing the diff algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageOptions {
    pub padding_x: u32,
    pub padding_y: u32,
    pub align_x: u32,
    pub align_y: u32,
}

impl Default for DamageOptions {
    fn default() -> Self {
        Self {
            padding_x: 0,
            padding_y: 0,
            align_x: 1,
            align_y: 1,
        }
    }
}

/// Find the smallest rectangle containing every changed RGB565 pixel.
///
/// `current` and `previous` are tightly packed visible-screen images with two
/// bytes per pixel. The scan is deliberately independent of the semantic
/// dirty hint: an unexpected change outside that hint must not be silently
/// omitted from the panel update.
pub fn changed_region(
    current: &[u8],
    previous: &[u8],
    width: u32,
    height: u32,
    options: DamageOptions,
) -> io::Result<Option<DisplayRegion>> {
    let width = usize::try_from(width)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid damage width"))?;
    let height = usize::try_from(height)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid damage height"))?;
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(2))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "damage image is too large"))?;
    if current.len() != expected || previous.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "damage images are {} and {} bytes, expected {expected}",
                current.len(),
                previous.len()
            ),
        ));
    }
    if width == 0 || height == 0 {
        return Ok(None);
    }

    let mut left = width;
    let mut top = height;
    let mut right = 0usize;
    let mut bottom = 0usize;
    for y in 0..height {
        let row_start = y * width * 2;
        for x in 0..width {
            let offset = row_start + x * 2;
            if current[offset..offset + 2] == previous[offset..offset + 2] {
                continue;
            }
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
    }
    if left == width {
        return Ok(None);
    }

    let region = DisplayRegion::new(
        u32::try_from(left).unwrap_or(u32::MAX),
        u32::try_from(top).unwrap_or(u32::MAX),
        u32::try_from(right - left).unwrap_or(u32::MAX),
        u32::try_from(bottom - top).unwrap_or(u32::MAX),
    );
    Ok(Some(expand_region(
        region,
        width as u32,
        height as u32,
        options,
    )))
}

pub fn expand_region(
    region: DisplayRegion,
    width: u32,
    height: u32,
    options: DamageOptions,
) -> DisplayRegion {
    let region = region.bounded(width, height);
    if region.width == 0 || region.height == 0 {
        return region;
    }

    let align_x = options.align_x.max(1);
    let align_y = options.align_y.max(1);
    let left = region.left.saturating_sub(options.padding_x) / align_x * align_x;
    let top = region.top.saturating_sub(options.padding_y) / align_y * align_y;
    let right = align_up(
        region
            .left
            .saturating_add(region.width)
            .saturating_add(options.padding_x),
        align_x,
    )
    .min(width);
    let bottom = align_up(
        region
            .top
            .saturating_add(region.height)
            .saturating_add(options.padding_y),
        align_y,
    )
    .min(height);

    DisplayRegion::new(
        left,
        top,
        right.saturating_sub(left),
        bottom.saturating_sub(top),
    )
}

fn align_up(value: u32, alignment: u32) -> u32 {
    if alignment <= 1 {
        return value;
    }
    let remainder = value % alignment;
    if remainder == 0 {
        value
    } else {
        value.saturating_add(alignment - remainder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: usize, height: usize, pixel: u16) -> Vec<u8> {
        let mut image = vec![0; width * height * 2];
        for chunk in image.chunks_exact_mut(2) {
            chunk.copy_from_slice(&pixel.to_ne_bytes());
        }
        image
    }

    fn set_pixel(image: &mut [u8], width: usize, x: usize, y: usize, pixel: u16) {
        let offset = (y * width + x) * 2;
        image[offset..offset + 2].copy_from_slice(&pixel.to_ne_bytes());
    }

    #[test]
    fn finds_exact_changed_pixel_bounds() {
        let mut current = image(8, 6, 0xffff);
        let previous = current.clone();
        set_pixel(&mut current, 8, 2, 1, 0);
        set_pixel(&mut current, 8, 5, 4, 0);

        assert_eq!(
            changed_region(&current, &previous, 8, 6, DamageOptions::default()).unwrap(),
            Some(DisplayRegion::new(2, 1, 4, 4))
        );
    }

    #[test]
    fn compares_both_rgb565_bytes_and_suppresses_identical_frames() {
        let mut current = image(2, 2, 0xffff);
        let mut previous = current.clone();
        assert_eq!(
            changed_region(&current, &previous, 2, 2, DamageOptions::default()).unwrap(),
            None
        );
        set_pixel(&mut current, 2, 1, 1, 0x00f8);
        assert_eq!(
            changed_region(&current, &previous, 2, 2, DamageOptions::default()).unwrap(),
            Some(DisplayRegion::new(1, 1, 1, 1))
        );
        previous = current.clone();
        assert_eq!(
            changed_region(&current, &previous, 2, 2, DamageOptions::default()).unwrap(),
            None
        );
    }

    #[test]
    fn expands_with_padding_and_alignment_at_edges() {
        let region = expand_region(
            DisplayRegion::new(5, 7, 3, 4),
            16,
            16,
            DamageOptions {
                padding_x: 2,
                padding_y: 1,
                align_x: 4,
                align_y: 4,
            },
        );
        assert_eq!(region, DisplayRegion::new(0, 4, 12, 8));

        let edge = expand_region(
            DisplayRegion::new(14, 14, 2, 2),
            16,
            16,
            DamageOptions {
                padding_x: 3,
                padding_y: 3,
                align_x: 8,
                align_y: 8,
            },
        );
        assert_eq!(edge, DisplayRegion::new(8, 8, 8, 8));
    }

    #[test]
    fn rejects_wrong_image_sizes() {
        let error = changed_region(&[0; 2], &[0; 2], 2, 2, DamageOptions::default()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
