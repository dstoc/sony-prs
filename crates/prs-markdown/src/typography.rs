//! Hardware-independent typography and glyph rasterization.
//!
//! [`FontdueTextEngine`] is the first implementation of [`TextEngine`].  The
//! rest of the reader only sees the types in this module, so a shaping/layout
//! implementation such as `cosmic-text` can replace it without leaking a
//! backend's types into parsing, pagination, or rendering.
//!
//! The current backend performs character-based layout.  It is intended for
//! Latin and code-heavy documents: complex-script shaping, bidirectional text,
//! grapheme-aware cursoring, and broad fallback font selection are not
//! supported yet.

use crate::layout::TextMeasurer;
use crate::style::TextStyle as ReaderTextStyle;
use fontdue::layout::{CoordinateSystem, LayoutSettings, WrapStyle};
use std::collections::VecDeque;
use std::fmt;

/// The font face used by a text run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontFace {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Monospace,
}

impl FontFace {
    const fn index(self) -> usize {
        match self {
            Self::Regular => 0,
            Self::Bold => 1,
            Self::Italic => 2,
            Self::BoldItalic => 3,
            Self::Monospace => 4,
        }
    }

    const fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Regular,
            1 => Self::Bold,
            2 => Self::Italic,
            3 => Self::BoldItalic,
            _ => Self::Monospace,
        }
    }
}

/// A font size and face applied to one text run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextStyle {
    pub face: FontFace,
    pub font_size: u32,
}

impl TextStyle {
    pub const fn new(face: FontFace, font_size: u32) -> Self {
        Self { face, font_size }
    }

    pub const fn regular(font_size: u32) -> Self {
        Self::new(FontFace::Regular, font_size)
    }

    pub const fn bold(font_size: u32) -> Self {
        Self::new(FontFace::Bold, font_size)
    }

    pub const fn italic(font_size: u32) -> Self {
        Self::new(FontFace::Italic, font_size)
    }

    pub const fn bold_italic(font_size: u32) -> Self {
        Self::new(FontFace::BoldItalic, font_size)
    }

    pub const fn monospace(font_size: u32) -> Self {
        Self::new(FontFace::Monospace, font_size)
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        Self::regular(16)
    }
}

/// Application-defined semantic metadata carried by each positioned glyph.
pub type SpanId = u32;

/// A borrowed run of styled text and its optional semantic span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextRun<'a> {
    pub text: &'a str,
    pub style: TextStyle,
    pub span_id: Option<SpanId>,
}

impl<'a> TextRun<'a> {
    pub const fn new(text: &'a str, style: TextStyle) -> Self {
        Self {
            text,
            style,
            span_id: None,
        }
    }

    pub const fn with_span_id(text: &'a str, style: TextStyle, span_id: SpanId) -> Self {
        Self {
            text,
            style,
            span_id: Some(span_id),
        }
    }

    pub const fn span(mut self, span_id: SpanId) -> Self {
        self.span_id = Some(span_id);
        self
    }
}

/// Integer line metrics in a positive-Y-down coordinate system.
///
/// `ascent` is the number of pixels above the baseline, while `descent` is
/// normally negative and represents pixels below it. `baseline` is the
/// baseline's offset from the top of the line. The invariant
/// `line_height >= ascent - descent + line_gap` is maintained by the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineMetrics {
    pub ascent: i32,
    pub descent: i32,
    pub line_gap: i32,
    pub line_height: u32,
    pub baseline: i32,
}

impl LineMetrics {
    fn from_font(font: &fontdue::Font, font_size: u32) -> Self {
        let fallback = font_size.max(1) as i32;
        let Some(metrics) = font.horizontal_line_metrics(font_size.max(1) as f32) else {
            return Self {
                ascent: fallback,
                descent: 0,
                line_gap: 0,
                line_height: fallback as u32,
                baseline: fallback,
            };
        };

        let ascent = metrics.ascent.ceil() as i32;
        let descent = metrics.descent.ceil() as i32;
        let line_gap = metrics.line_gap.ceil().max(0.0) as i32;
        let line_height = (metrics.new_line_size.ceil() as i32)
            .max(ascent.saturating_sub(descent).saturating_add(line_gap))
            .max(1) as u32;
        Self {
            ascent,
            descent,
            line_gap,
            line_height,
            baseline: ascent.max(0),
        }
    }
}

/// Width and vertical metrics for one run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextMetrics {
    /// The unrounded horizontal advance in pixels.
    pub advance_width: f32,
    /// The advance rounded up for integer display-list coordinates.
    pub width: u32,
    pub line: LineMetrics,
}

/// A line of laid-out text. Glyphs for this line are in `glyphs[glyph_range]`.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub glyph_range: std::ops::Range<usize>,
    pub width: u32,
    pub metrics: LineMetrics,
}

/// The result of wrapping one or more styled runs to a width.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextLayout {
    pub lines: Vec<TextLine>,
    pub glyphs: Vec<PositionedGlyph>,
}

impl TextLayout {
    pub fn lines(&self) -> &[TextLine] {
        &self.lines
    }

    pub fn glyphs(&self) -> &[PositionedGlyph] {
        &self.glyphs
    }
}

/// A backend-neutral positioned glyph.
#[derive(Clone, Debug, PartialEq)]
pub struct PositionedGlyph {
    pub character: char,
    pub glyph_id: u16,
    pub face: FontFace,
    pub font_size: u32,
    /// The top-left pixel position of the glyph bitmap relative to the layout origin.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub advance_width: f32,
    pub byte_offset: usize,
    pub span_id: Option<SpanId>,
}

/// A rasterized glyph bitmap and the offsets needed to place it at its baseline.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    /// Horizontal bitmap offset from the glyph origin.
    pub left: i32,
    /// Vertical bitmap offset from the baseline in positive-Y-down coordinates.
    pub top: i32,
    pub advance_width: f32,
    /// One 8-bit coverage value per pixel, row-major from the top-left corner.
    pub alpha: Vec<u8>,
}

/// The shaping/rasterization boundary used by the reader.
pub trait TextEngine {
    fn measure(&self, run: &TextRun<'_>) -> TextMetrics;
    fn wrap(&self, runs: &[TextRun<'_>], available_width: u32) -> TextLayout;
    fn rasterize_glyph(&mut self, glyph: &PositionedGlyph) -> GlyphBitmap;
}

/// Font parser settings exposed without exposing any Fontdue types.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontLoadConfig {
    pub collection_index: u32,
    pub geometry_scale: f32,
    pub load_substitutions: bool,
}

impl Default for FontLoadConfig {
    fn default() -> Self {
        Self {
            collection_index: 0,
            geometry_scale: 40.0,
            load_substitutions: true,
        }
    }
}

/// Caller-supplied font bytes for the five reader faces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontConfig {
    regular: Vec<u8>,
    bold: Vec<u8>,
    italic: Vec<u8>,
    bold_italic: Vec<u8>,
    monospace: Vec<u8>,
}

impl FontConfig {
    /// Use one supplied family for every face. This is useful for a harness
    /// that wants a small configuration while retaining explicit face
    /// selection in the layout API.
    pub fn from_regular(bytes: impl AsRef<[u8]>) -> Self {
        let bytes = bytes.as_ref().to_vec();
        Self {
            regular: bytes.clone(),
            bold: bytes.clone(),
            italic: bytes.clone(),
            bold_italic: bytes.clone(),
            monospace: bytes,
        }
    }

    /// Supply independent bytes for every supported face.
    pub fn from_faces(
        regular: impl AsRef<[u8]>,
        bold: impl AsRef<[u8]>,
        italic: impl AsRef<[u8]>,
        bold_italic: impl AsRef<[u8]>,
        monospace: impl AsRef<[u8]>,
    ) -> Self {
        Self {
            regular: regular.as_ref().to_vec(),
            bold: bold.as_ref().to_vec(),
            italic: italic.as_ref().to_vec(),
            bold_italic: bold_italic.as_ref().to_vec(),
            monospace: monospace.as_ref().to_vec(),
        }
    }

    fn bytes(&self, face: FontFace) -> &[u8] {
        match face {
            FontFace::Regular => &self.regular,
            FontFace::Bold => &self.bold,
            FontFace::Italic => &self.italic,
            FontFace::BoldItalic => &self.bold_italic,
            FontFace::Monospace => &self.monospace,
        }
    }
}

/// An invalid caller-supplied face.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontError {
    pub face: FontFace,
    pub message: &'static str,
}

impl fmt::Display for FontError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {:?} font: {}", self.face, self.message)
    }
}

impl std::error::Error for FontError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct GlyphCacheKey {
    face: FontFace,
    glyph_id: u16,
    font_size: u32,
}

#[derive(Clone, Debug)]
struct GlyphCache {
    capacity: usize,
    entries: VecDeque<(GlyphCacheKey, GlyphBitmap)>,
}

impl GlyphCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::new(),
        }
    }

    fn get(&mut self, key: GlyphCacheKey) -> Option<GlyphBitmap> {
        let index = self
            .entries
            .iter()
            .position(|(entry_key, _)| *entry_key == key)?;
        let entry = self.entries.remove(index)?;
        let bitmap = entry.1.clone();
        self.entries.push_back(entry);
        Some(bitmap)
    }

    fn insert(&mut self, key: GlyphCacheKey, bitmap: GlyphBitmap) {
        if self.capacity == 0 {
            return;
        }
        if let Some(index) = self
            .entries
            .iter()
            .position(|(entry_key, _)| *entry_key == key)
        {
            self.entries.remove(index);
        }
        while self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back((key, bitmap));
    }
}

/// Fontdue-backed implementation of [`TextEngine`].
#[derive(Clone, Debug)]
pub struct FontdueTextEngine {
    fonts: [fontdue::Font; 5],
    cache: GlyphCache,
    cache_hits: u64,
    rasterizations: u64,
}

impl FontdueTextEngine {
    /// Load all five faces and reserve at most `cache_capacity` rasterized
    /// glyphs. A capacity of zero disables reuse but remains bounded.
    pub fn new(config: FontConfig, cache_capacity: usize) -> Result<Self, FontError> {
        Self::with_load_config(config, FontLoadConfig::default(), cache_capacity)
    }

    pub fn with_load_config(
        config: FontConfig,
        load_config: FontLoadConfig,
        cache_capacity: usize,
    ) -> Result<Self, FontError> {
        let settings = fontdue::FontSettings {
            collection_index: load_config.collection_index,
            scale: load_config.geometry_scale,
            load_substitutions: load_config.load_substitutions,
        };
        let fonts = [
            load_font(FontFace::Regular, config.bytes(FontFace::Regular), settings)?,
            load_font(FontFace::Bold, config.bytes(FontFace::Bold), settings)?,
            load_font(FontFace::Italic, config.bytes(FontFace::Italic), settings)?,
            load_font(
                FontFace::BoldItalic,
                config.bytes(FontFace::BoldItalic),
                settings,
            )?,
            load_font(
                FontFace::Monospace,
                config.bytes(FontFace::Monospace),
                settings,
            )?,
        ];
        Ok(Self {
            fonts,
            cache: GlyphCache::new(cache_capacity),
            cache_hits: 0,
            rasterizations: 0,
        })
    }

    pub fn measure_text(&self, text: &str, style: TextStyle) -> TextMetrics {
        TextEngine::measure(self, &TextRun::new(text, style))
    }

    pub fn layout(&self, runs: &[TextRun<'_>], available_width: u32) -> TextLayout {
        self.wrap(runs, available_width)
    }

    pub fn cache_capacity(&self) -> usize {
        self.cache.capacity
    }

    pub fn cached_glyphs(&self) -> usize {
        self.cache.entries.len()
    }

    pub fn cache_hits(&self) -> u64 {
        self.cache_hits
    }

    pub fn rasterized_glyphs(&self) -> u64 {
        self.rasterizations
    }

    pub fn clear_cache(&mut self) {
        self.cache.entries.clear();
    }

    fn font(&self, face: FontFace) -> &fontdue::Font {
        &self.fonts[face.index()]
    }

    fn fontdue_layout(
        &self,
        runs: &[TextRun<'_>],
        available_width: u32,
    ) -> fontdue::layout::Layout<Option<SpanId>> {
        let mut layout = fontdue::layout::Layout::new(CoordinateSystem::PositiveYDown);
        layout.reset(&LayoutSettings {
            max_width: Some(available_width.max(1) as f32),
            wrap_style: WrapStyle::Word,
            wrap_hard_breaks: true,
            ..LayoutSettings::default()
        });
        for run in runs {
            let style = fontdue::layout::TextStyle::with_user_data(
                run.text,
                run.style.font_size.max(1) as f32,
                run.style.face.index(),
                run.span_id,
            );
            layout.append(&self.fonts, &style);
        }
        layout
    }

    fn rasterize_uncached(&self, key: GlyphCacheKey) -> GlyphBitmap {
        let font = self.font(key.face);
        let glyph_id = key.glyph_id.min(font.glyph_count().saturating_sub(1));
        let (metrics, alpha) = font.rasterize_indexed(glyph_id, key.font_size.max(1) as f32);
        GlyphBitmap {
            width: metrics.width as u32,
            height: metrics.height as u32,
            left: metrics.xmin,
            top: -(metrics.height as i32) - metrics.ymin,
            advance_width: metrics.advance_width,
            alpha,
        }
    }
}

fn load_font(
    face: FontFace,
    bytes: &[u8],
    settings: fontdue::FontSettings,
) -> Result<fontdue::Font, FontError> {
    fontdue::Font::from_bytes(bytes, settings).map_err(|message| FontError { face, message })
}

impl TextEngine for FontdueTextEngine {
    fn measure(&self, run: &TextRun<'_>) -> TextMetrics {
        let font = self.font(run.style.face);
        let font_size = run.style.font_size.max(1) as f32;
        let mut max_advance: f32 = 0.0;
        let mut advance: f32 = 0.0;
        let mut width: u32 = 0;
        let mut max_width: u32 = 0;
        for character in run.text.chars() {
            if character == '\n' {
                max_advance = max_advance.max(advance);
                max_width = max_width.max(width);
                advance = 0.0;
                width = 0;
                continue;
            }
            let metrics = font.metrics(character, font_size);
            advance += metrics.advance_width;
            // Fontdue's basic layout advances one whole pixel per glyph. Keep
            // standalone measurement aligned with the wrapping implementation.
            width = width.saturating_add(metrics.advance_width.ceil().max(0.0) as u32);
        }
        max_advance = max_advance.max(advance);
        max_width = max_width.max(width);
        TextMetrics {
            advance_width: max_advance,
            width: max_width,
            line: LineMetrics::from_font(font, run.style.font_size),
        }
    }

    fn wrap(&self, runs: &[TextRun<'_>], available_width: u32) -> TextLayout {
        let layout = self.fontdue_layout(runs, available_width);
        let glyphs = layout
            .glyphs()
            .iter()
            .map(|glyph| {
                let face = FontFace::from_index(glyph.font_index);
                let font_size = glyph.key.px.round().max(1.0) as u32;
                let metrics = self
                    .font(face)
                    .metrics_indexed(glyph.key.glyph_index, font_size as f32);
                PositionedGlyph {
                    character: glyph.parent,
                    glyph_id: glyph.key.glyph_index,
                    face,
                    font_size,
                    x: glyph.x.round() as i32,
                    y: glyph.y.round() as i32,
                    width: glyph.width as u32,
                    height: glyph.height as u32,
                    advance_width: metrics.advance_width,
                    byte_offset: glyph.byte_offset,
                    span_id: glyph.user_data,
                }
            })
            .collect::<Vec<_>>();

        let lines = layout
            .lines()
            .into_iter()
            .flatten()
            .map(|line| {
                let start = line.glyph_start.min(glyphs.len());
                let end = line
                    .glyph_end
                    .saturating_add(1)
                    .min(glyphs.len())
                    .max(start);
                let width = if line.padding.is_finite() {
                    (available_width as f32 - line.padding).max(0.0).ceil() as u32
                } else {
                    glyphs[start..end]
                        .iter()
                        .map(|glyph| glyph.x.saturating_add(glyph.width as i32).max(0) as u32)
                        .max()
                        .unwrap_or(0)
                };
                let metrics = LineMetrics {
                    ascent: line.max_ascent as i32,
                    descent: line.min_descent as i32,
                    line_gap: line.max_line_gap.max(0.0) as i32,
                    line_height: line.max_new_line_size.max(1.0) as u32,
                    baseline: line.baseline_y.round() as i32,
                };
                TextLine {
                    glyph_range: start..end,
                    width,
                    metrics,
                }
            })
            .collect();

        TextLayout { lines, glyphs }
    }

    fn rasterize_glyph(&mut self, glyph: &PositionedGlyph) -> GlyphBitmap {
        let key = GlyphCacheKey {
            face: glyph.face,
            glyph_id: glyph.glyph_id,
            font_size: glyph.font_size.max(1),
        };
        if let Some(bitmap) = self.cache.get(key) {
            self.cache_hits += 1;
            return bitmap;
        }
        let bitmap = self.rasterize_uncached(key);
        self.rasterizations += 1;
        self.cache.insert(key, bitmap.clone());
        bitmap
    }
}

impl TextMeasurer for FontdueTextEngine {
    fn measure(&self, text: &str, style: &ReaderTextStyle) -> u32 {
        let face = if style.code {
            FontFace::Monospace
        } else if style.bold && style.italic {
            FontFace::BoldItalic
        } else if style.bold {
            FontFace::Bold
        } else if style.italic {
            FontFace::Italic
        } else {
            FontFace::Regular
        };
        self.measure_text(text, TextStyle::new(face, style.font_size))
            .width
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font_bytes() -> Vec<u8> {
        [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
        ]
        .iter()
        .find_map(|path| std::fs::read(path).ok())
        .expect("host typography tests need a system TrueType font")
    }

    fn monospace_bytes() -> Vec<u8> {
        std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf")
            .expect("host typography tests need a system monospace TrueType font")
    }

    fn engine(cache_capacity: usize) -> FontdueTextEngine {
        let regular = font_bytes();
        FontdueTextEngine::new(
            FontConfig::from_faces(&regular, &regular, &regular, &regular, monospace_bytes()),
            cache_capacity,
        )
        .expect("test fonts should parse")
    }

    #[test]
    fn proportional_measurement_distinguishes_narrow_and_wide_runs() {
        let engine = engine(8);
        let narrow = engine.measure_text("iii", TextStyle::regular(20));
        let wide = engine.measure_text("WWW", TextStyle::regular(20));

        assert!(wide.advance_width > narrow.advance_width);
        assert!(wide.width > narrow.width);
    }

    #[test]
    fn wrapping_respects_available_width_and_keeps_span_metadata() {
        let engine = engine(8);
        let runs = [
            TextRun::with_span_id("one two", TextStyle::regular(18), 7),
            TextRun::new(" ", TextStyle::bold(18)),
            TextRun::new("three", TextStyle::italic(18)),
        ];
        let layout = engine.wrap(&runs, 70);

        assert!(layout.lines.len() > 1);
        assert!(layout.lines.iter().all(|line| line.width <= 70));
        assert!(layout.glyphs.iter().any(|glyph| glyph.span_id == Some(7)));
        assert!(layout
            .glyphs
            .iter()
            .any(|glyph| glyph.face == FontFace::Bold));
        assert!(layout
            .glyphs
            .iter()
            .any(|glyph| glyph.face == FontFace::Italic));
    }

    #[test]
    fn monospace_measurement_has_equal_character_advances() {
        let engine = engine(8);
        let style = TextStyle::monospace(20);
        let one = engine.measure_text("i", style);
        let other = engine.measure_text("W", style);

        assert_eq!(one.advance_width, other.advance_width);
    }

    #[test]
    fn line_height_and_baseline_are_consistent() {
        let engine = engine(8);
        let layout = engine.wrap(&[TextRun::new("Hello", TextStyle::regular(20))], 200);
        let line = &layout.lines[0];

        assert_eq!(line.metrics.baseline, line.metrics.ascent);
        assert!(
            line.metrics.line_height as i32
                >= line.metrics.ascent - line.metrics.descent + line.metrics.line_gap
        );
        assert!(layout.glyphs[0].y < line.metrics.baseline);
    }

    #[test]
    fn raster_cache_reuses_glyphs_and_stays_bounded() {
        let mut engine = engine(2);
        let layout = engine.wrap(
            &[
                TextRun::new("ab", TextStyle::regular(20)),
                TextRun::new("c", TextStyle::regular(20)),
            ],
            200,
        );
        let first = engine.rasterize_glyph(&layout.glyphs[0]);
        let second = engine.rasterize_glyph(&layout.glyphs[0]);
        assert_eq!(first, second);
        assert_eq!(engine.rasterized_glyphs(), 1);
        assert_eq!(engine.cache_hits(), 1);

        for glyph in &layout.glyphs {
            engine.rasterize_glyph(glyph);
        }
        assert!(engine.cached_glyphs() <= engine.cache_capacity());
    }
}
