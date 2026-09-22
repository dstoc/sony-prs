//! Bounded, device-independent raster image loading.
//!
//! Image decoding belongs here rather than in Markdown parsing or a T1
//! framebuffer adapter. Encoded bytes are decoded only long enough to resize
//! them to the reader's display area; the retained representation is an
//! opaque, grayscale raster suitable for an e-ink-oriented renderer.

use crate::document::{Block, Document, Inline, ListItem};
use crate::resources::{ResourceProvider, ResourceTarget};
use image::{DynamicImage, ImageReader, Limits};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

/// The largest allocation allowed while decoding one encoded source image.
/// Images are still resized to the display area before they are retained.
pub const MAX_SOURCE_ALLOCATION: u64 = 64 * 1024 * 1024;

/// The largest encoded image source read into memory by the resource layer.
/// This is intentionally lower than the decoder allocation limit because the
/// encoded byte buffer and decoder working memory coexist during decode.
pub const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;

/// The total display-sized raster data retained by one [`ImageResources`].
/// Images beyond this budget degrade to the normal alt-text fallback.
pub const DEFAULT_RETAINED_BYTES: usize = 4 * 1024 * 1024;

/// Maximum number of distinct image references retained in one document.
/// Sources after this bound use the normal alt-text fallback.
pub const DEFAULT_IMAGE_ENTRIES: usize = 128;

/// A display-sized, opaque 8-bit grayscale raster.
///
/// Pixels are row-major and composited against white during loading. An
/// [`Arc`] lets repeated references share their bounded display data without
/// retaining the decoded source image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RasterImage {
    width: u32,
    height: u32,
    pixels: Arc<[u8]>,
}

impl RasterImage {
    pub fn new(width: u32, height: u32, pixels: impl Into<Arc<[u8]>>) -> Option<Self> {
        let expected = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        let pixels = pixels.into();
        (width > 0 && height > 0 && pixels.len() == expected).then_some(Self {
            width,
            height,
            pixels,
        })
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

    pub fn pixel(&self, x: u32, y: u32) -> Option<u8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = usize::try_from(y)
            .ok()?
            .checked_mul(usize::try_from(self.width).ok()?)?
            .checked_add(usize::try_from(x).ok()?)?;
        self.pixels.get(index).copied()
    }

    pub fn byte_len(&self) -> usize {
        self.pixels.len()
    }

    /// Return a proportionally fitted copy without upscaling.
    pub fn fitted(&self, max_width: u32, max_height: u32) -> Self {
        let max_width = max_width.max(1);
        let max_height = max_height.max(1);
        if self.width <= max_width && self.height <= max_height {
            return self.clone();
        }

        let (width, height) = fitted_dimensions(self.width, self.height, max_width, max_height);
        if width == self.width && height == self.height {
            return self.clone();
        }

        let source = image::GrayImage::from_raw(self.width, self.height, self.pixels.to_vec())
            .expect("RasterImage dimensions and pixels are validated");
        let resized = image::imageops::resize(
            &source,
            width,
            height,
            image::imageops::FilterType::Triangle,
        );
        Self::new(width, height, Arc::<[u8]>::from(resized.into_raw()))
            .expect("resized image dimensions and pixels are valid")
    }
}

/// Display data for all image references in one Markdown document.
///
/// Failed resolution, unsupported formats, corrupt bytes, and images that
/// exceed the retained-data budget are intentionally recorded as unavailable
/// rather than returned as document-loading errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageResources {
    images: HashMap<String, Option<RasterImage>>,
    retained_bytes: usize,
    retained_limit: usize,
    entry_limit: usize,
}

impl Default for ImageResources {
    fn default() -> Self {
        Self::new(DEFAULT_RETAINED_BYTES)
    }
}

impl ImageResources {
    pub fn new(retained_limit: usize) -> Self {
        Self::with_limits(retained_limit, DEFAULT_IMAGE_ENTRIES)
    }

    pub fn with_limits(retained_limit: usize, entry_limit: usize) -> Self {
        Self {
            images: HashMap::new(),
            retained_bytes: 0,
            retained_limit,
            entry_limit,
        }
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn retained_limit(&self) -> usize {
        self.retained_limit
    }

    pub fn entry_count(&self) -> usize {
        self.images.len()
    }

    pub fn entry_limit(&self) -> usize {
        self.entry_limit
    }

    pub fn image(&self, source: &str) -> Option<&RasterImage> {
        self.images.get(source).and_then(Option::as_ref)
    }

    pub fn contains(&self, source: &str) -> bool {
        self.images.contains_key(source)
    }

    pub fn insert(&mut self, source: impl Into<String>, image: Option<RasterImage>) {
        let source = source.into();
        if !self.images.contains_key(&source) && self.images.len() >= self.entry_limit {
            return;
        }
        if let Some(previous) = self.images.remove(&source).flatten() {
            self.retained_bytes = self.retained_bytes.saturating_sub(previous.byte_len());
        }
        let image = image.filter(|image| {
            image.byte_len() <= self.retained_limit
                && image.byte_len() <= self.retained_limit.saturating_sub(self.retained_bytes)
        });
        if let Some(image) = &image {
            self.retained_bytes = self.retained_bytes.saturating_add(image.byte_len());
        }
        self.images.insert(source, image);
    }

    /// Resolve and load every image referenced by `document`.
    pub fn from_document<P: ResourceProvider>(
        provider: &P,
        containing_document: &Path,
        document: &Document,
        max_width: u32,
        max_height: u32,
    ) -> Self {
        Self::from_document_with_limit(
            provider,
            containing_document,
            document,
            max_width,
            max_height,
            DEFAULT_RETAINED_BYTES,
        )
    }

    pub fn from_document_with_limit<P: ResourceProvider>(
        provider: &P,
        containing_document: &Path,
        document: &Document,
        max_width: u32,
        max_height: u32,
        retained_limit: usize,
    ) -> Self {
        Self::from_document_with_limits(
            provider,
            containing_document,
            document,
            max_width,
            max_height,
            retained_limit,
            DEFAULT_IMAGE_ENTRIES,
        )
    }

    pub fn from_document_with_limits<P: ResourceProvider>(
        provider: &P,
        containing_document: &Path,
        document: &Document,
        max_width: u32,
        max_height: u32,
        retained_limit: usize,
        entry_limit: usize,
    ) -> Self {
        let mut sources = Vec::new();
        for block in document.blocks() {
            collect_block_sources(block, &mut sources, entry_limit);
        }

        let mut resources = Self::with_limits(retained_limit, entry_limit);
        for source in sources {
            let image = load_reference(
                provider,
                containing_document,
                &source,
                max_width,
                max_height,
            );
            resources.insert(source, image);
        }
        resources
    }
}

/// Decode an encoded image to bounded grayscale display data.
pub fn decode(bytes: &[u8], max_width: u32, max_height: u32) -> Option<RasterImage> {
    let dimensions = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    if dimensions.0 == 0 || dimensions.1 == 0 {
        return None;
    }

    // These limits protect the transient decoder allocation as well as the
    // final retained raster. The display-size budget is much smaller, but
    // encoded formats must be decoded before they can be resized.
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(MAX_SOURCE_ALLOCATION);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    Some(rasterize(decoded, max_width, max_height))
}

fn load_reference<P: ResourceProvider>(
    provider: &P,
    containing_document: &Path,
    source: &str,
    max_width: u32,
    max_height: u32,
) -> Option<RasterImage> {
    let target = provider
        .resolve_reference_from(containing_document, source)
        .ok()?;
    let ResourceTarget::Asset(path) = target else {
        return None;
    };
    let bytes = provider.read_binary_limited(&path, MAX_SOURCE_BYTES).ok()?;
    decode(&bytes, max_width, max_height)
}

fn rasterize(decoded: DynamicImage, max_width: u32, max_height: u32) -> RasterImage {
    let (width, height) = fitted_dimensions(
        decoded.width(),
        decoded.height(),
        max_width.max(1),
        max_height.max(1),
    );
    let rgba = if width != decoded.width() || height != decoded.height() {
        decoded.resize_exact(width, height, image::imageops::FilterType::Triangle)
    } else {
        decoded
    }
    .to_rgba8();
    let pixels = rgba
        .pixels()
        .map(|pixel| {
            let [red, green, blue, alpha] = pixel.0;
            let luminance =
                (u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114 + 500)
                    / 1000;
            ((luminance * u32::from(alpha) + 255 * u32::from(255u8.saturating_sub(alpha)) + 127)
                / 255) as u8
        })
        .collect::<Vec<_>>();
    RasterImage::new(width, height, Arc::<[u8]>::from(pixels))
        .expect("decoded image dimensions and pixels are valid")
}

fn fitted_dimensions(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let max_width = max_width.max(1);
    let max_height = max_height.max(1);
    if width <= max_width && height <= max_height {
        return (width.max(1), height.max(1));
    }

    let width_limited_height = u64::from(height)
        .saturating_mul(u64::from(max_width))
        .checked_div(u64::from(width).max(1))
        .unwrap_or(1)
        .max(1) as u32;
    if width_limited_height <= max_height {
        (max_width, width_limited_height)
    } else {
        let height_limited_width = u64::from(width)
            .saturating_mul(u64::from(max_height))
            .checked_div(u64::from(height).max(1))
            .unwrap_or(1)
            .max(1) as u32;
        (height_limited_width, max_height)
    }
}

fn collect_block_sources(block: &Block, output: &mut Vec<String>, limit: usize) {
    match block {
        Block::Heading { content, .. } | Block::Paragraph(content) => {
            collect_inline_sources(content, output, limit)
        }
        Block::List { items, .. } => items
            .iter()
            .for_each(|item| collect_item_sources(item, output, limit)),
        Block::Quote(blocks)
        | Block::Alert { blocks, .. }
        | Block::FootnoteDefinition { blocks, .. } => blocks
            .iter()
            .for_each(|block| collect_block_sources(block, output, limit)),
        Block::Table(table) => {
            table
                .headers
                .iter()
                .for_each(|cell| collect_inline_sources(cell, output, limit));
            table
                .rows
                .iter()
                .flatten()
                .for_each(|cell| collect_inline_sources(cell, output, limit));
        }
        Block::Image { source, .. } => push_source(output, source, limit),
        Block::CodeBlock { .. } | Block::Rule => {}
    }
}

fn collect_item_sources(item: &ListItem, output: &mut Vec<String>, limit: usize) {
    collect_inline_sources(&item.content, output, limit);
    item.children
        .iter()
        .for_each(|block| collect_block_sources(block, output, limit));
}

fn collect_inline_sources(inlines: &[Inline], output: &mut Vec<String>, limit: usize) {
    for inline in inlines {
        match inline {
            Inline::Image { source, .. } => push_source(output, source, limit),
            Inline::Emphasis(children)
            | Inline::Strong(children)
            | Inline::Strikethrough(children) => collect_inline_sources(children, output, limit),
            Inline::Link { label, .. } => collect_inline_sources(label, output, limit),
            Inline::Text(_)
            | Inline::Code(_)
            | Inline::FootnoteReference { .. }
            | Inline::SoftBreak
            | Inline::HardBreak => {}
        }
    }
}

fn push_source(output: &mut Vec<String>, source: &str, limit: usize) {
    if output.len() < limit && !output.iter().any(|existing| existing == source) {
        output.push(source.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageFormat, Rgba};

    fn encoded(format: ImageFormat) -> Vec<u8> {
        let image = ImageBuffer::from_fn(4, 2, |x, y| {
            if x < 2 && y == 0 {
                Rgba([0, 0, 0, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        });
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, format)
            .expect("test image encoding");
        bytes.into_inner()
    }

    #[test]
    fn supported_formats_decode_to_fitted_grayscale() {
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
            let image = decode(&encoded(format), 2, 2).expect("supported image");
            assert_eq!((image.width(), image.height()), (2, 1));
            assert_eq!(image.pixels().len(), 2);
            assert!(image.pixels().iter().any(|pixel| *pixel < 255));
        }
    }

    #[test]
    fn decode_keeps_fitted_dimensions_for_large_landscape_images() {
        let source = ImageBuffer::from_pixel(2600, 1520, Rgba([80, 120, 160, 255]));
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(source)
            .write_to(&mut bytes, ImageFormat::Png)
            .expect("test image encoding");

        let image = decode(bytes.get_ref(), 552, 676).expect("large image should decode");

        assert_eq!((image.width(), image.height()), (552, 322));
        assert_eq!(image.pixels().len(), 552 * 322);
    }

    #[test]
    fn corrupt_and_unsupported_bytes_are_recoverable() {
        assert!(decode(b"not an image", 100, 100).is_none());
        assert!(decode(b"GIF89a", 100, 100).is_none());
    }

    #[test]
    fn raster_fit_preserves_aspect_ratio_without_upscaling() {
        let image = RasterImage::new(4, 2, Arc::<[u8]>::from(vec![0; 8])).unwrap();
        assert_eq!(
            (image.fitted(8, 8).width(), image.fitted(8, 8).height()),
            (4, 2)
        );
        assert_eq!(
            (image.fitted(3, 8).width(), image.fitted(3, 8).height()),
            (3, 1)
        );
    }

    #[test]
    fn image_reference_metadata_is_bounded_separately_from_raster_bytes() {
        let raster = || RasterImage::new(1, 1, Arc::<[u8]>::from(vec![0])).unwrap();
        let mut resources = ImageResources::with_limits(16, 2);
        resources.insert("first", Some(raster()));
        resources.insert("second", Some(raster()));
        resources.insert("third", Some(raster()));

        assert_eq!(resources.entry_count(), 2);
        assert_eq!(resources.retained_bytes(), 2);
        assert!(resources.image("first").is_some());
        assert!(resources.image("third").is_none());
    }
}
