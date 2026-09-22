use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{OriginDimensions, Point, Size};
use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::prelude::Pixel;
use js_sys::{Array, Object, Reflect, Uint8Array};
use prs_markdown::parse::ComrakParser;
use prs_markdown::render::EmbeddedGraphicsRenderer;
use prs_markdown::resources::ResourceError;
use prs_markdown::typography::{FontConfig, FontdueTextEngine};
use prs_markdown::{
    BrowserResourceProvider, Reader, ReaderError, ReaderEvent, ReaderStyle, ResourceProvider,
    ResourceTarget, Viewport, T1_VIEWPORT,
};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// A reader backed by a snapshot of the directory selected in the browser.
///
/// The File System Access API is asynchronous, so JavaScript creates the
/// snapshot before calling [`load_directory`]. Once constructed, the shared
/// reader uses the same synchronous `ResourceProvider` boundary as native
/// readers.
#[wasm_bindgen]
pub struct BrowserReader {
    reader: Reader<BrowserResourceProvider>,
}

/// Build and open a reader from the files returned by `directory-library.js`.
///
/// Each array item must be an object with a relative `path` string and a
/// `Uint8Array` of `bytes`. The entry point defaults to `README.md` in the
/// browser UI, but callers may supply another root-relative Markdown path.
#[wasm_bindgen]
pub fn load_directory(files: Array, entry_point: String) -> Result<BrowserReader, JsValue> {
    let files = parse_files(files)?;
    let provider = BrowserResourceProvider::new(Path::new(&entry_point), files)
        .map_err(|error| structured_error("resource", error))?;
    let mut reader = Reader::new(
        provider,
        prs_markdown::ReaderStyle::default(),
        Viewport::new(600, 800),
    );
    reader
        .open()
        .map_err(|error| structured_reader_error("open", error))?;
    Ok(BrowserReader { reader })
}

#[wasm_bindgen]
impl BrowserReader {
    pub fn entry_point(&self) -> String {
        self.reader.provider().entry_point().display().to_string()
    }

    pub fn current_document(&self) -> String {
        self.reader
            .current_location()
            .map(|location| location.document.as_ref().to_owned())
            .unwrap_or_default()
    }

    pub fn page_count(&self) -> u32 {
        self.reader.page_count().try_into().unwrap_or(u32::MAX)
    }

    /// Resolve a local document or asset through the selected directory.
    ///
    /// This is useful to the simulator shell until its display renderer is
    /// wired to link hit regions. It also ensures browser navigation goes
    /// through the shared Rust resource policy.
    pub fn resolve_reference(&self, reference: &str) -> Result<JsValue, JsValue> {
        let containing_document = self
            .reader
            .current_location()
            .map(|location| PathBuf::from(location.document.as_ref()))
            .ok_or_else(|| structured_error("reader", "no document is open"))?;
        let target = self
            .reader
            .provider()
            .resolve_reference_from(&containing_document, reference)
            .map_err(|error| structured_error("resource", error))?;
        Ok(target_value(&target))
    }

    /// Follow a document, anchor, asset, or external URL reference.
    pub fn follow_reference(&mut self, reference: &str) -> Result<JsValue, JsValue> {
        let event = self
            .reader
            .follow_reference(reference)
            .map_err(|error| structured_reader_error("navigate", error))?;
        Ok(event_value(&event))
    }

    /// Read a selected-directory asset through the same root-relative checks
    /// used by Markdown image loading.
    pub fn read_asset(&self, path: &str) -> Result<Uint8Array, JsValue> {
        let bytes = self
            .reader
            .provider()
            .read_binary(Path::new(path))
            .map_err(|error| structured_error("resource", error))?;
        Ok(Uint8Array::from(bytes.as_slice()))
    }
}

fn parse_files(files: Array) -> Result<Vec<(PathBuf, Vec<u8>)>, JsValue> {
    files
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let path = Reflect::get(&value, &JsValue::from_str("path"))
                .map_err(|_| structured_error("bridge", format!("file {index} has no path")))?
                .as_string()
                .ok_or_else(|| {
                    structured_error("bridge", format!("file {index} path is not a string"))
                })?;
            let bytes = Reflect::get(&value, &JsValue::from_str("bytes"))
                .map_err(|_| structured_error("bridge", format!("file {index} has no bytes")))?
                .dyn_into::<Uint8Array>()
                .map_err(|_| {
                    structured_error("bridge", format!("file {index} bytes are not a Uint8Array"))
                })?;
            let mut copied = vec![0; bytes.length() as usize];
            bytes.copy_to(&mut copied);
            Ok((PathBuf::from(path), copied))
        })
        .collect()
}

fn structured_reader_error(kind: &str, error: ReaderError) -> JsValue {
    structured_error(kind, error)
}

fn structured_error(kind: &str, error: impl std::fmt::Display) -> JsValue {
    let object = Object::new();
    let _ = Reflect::set(
        &object,
        &JsValue::from_str("code"),
        &JsValue::from_str(&format!("reader_{kind}_error")),
    );
    let _ = Reflect::set(
        &object,
        &JsValue::from_str("message"),
        &JsValue::from_str(&error.to_string()),
    );
    object.into()
}

fn target_value(target: &ResourceTarget) -> JsValue {
    let object = Object::new();
    match target {
        ResourceTarget::Anchor(anchor) => set_target(&object, "anchor", None, Some(anchor)),
        ResourceTarget::Document(path) => set_target(&object, "document", Some(path), None),
        ResourceTarget::DocumentAnchor { document, anchor } => {
            set_target(&object, "document_anchor", Some(document), Some(anchor))
        }
        ResourceTarget::Asset(path) => set_target(&object, "asset", Some(path), None),
        ResourceTarget::External(url) => {
            let _ = Reflect::set(
                &object,
                &JsValue::from_str("kind"),
                &JsValue::from_str("external"),
            );
            let _ = Reflect::set(&object, &JsValue::from_str("url"), &JsValue::from_str(url));
        }
    }
    object.into()
}

fn set_target(object: &Object, kind: &str, path: Option<&Path>, anchor: Option<&String>) {
    let _ = Reflect::set(object, &JsValue::from_str("kind"), &JsValue::from_str(kind));
    if let Some(path) = path {
        let _ = Reflect::set(
            object,
            &JsValue::from_str("path"),
            &JsValue::from_str(&path.display().to_string()),
        );
    }
    if let Some(anchor) = anchor {
        let _ = Reflect::set(
            object,
            &JsValue::from_str("anchor"),
            &JsValue::from_str(anchor),
        );
    }
}

fn event_value(event: &ReaderEvent) -> JsValue {
    let object = Object::new();
    let (kind, path) = match event {
        ReaderEvent::Opened { location, .. }
        | ReaderEvent::Navigated { location, .. }
        | ReaderEvent::Back { location, .. }
        | ReaderEvent::Forward { location, .. } => {
            ("document", Some(location.document.as_ref().to_owned()))
        }
        ReaderEvent::PageChanged { .. } => ("page_changed", None),
        ReaderEvent::ExternalUrl(url) => {
            let _ = Reflect::set(
                &object,
                &JsValue::from_str("kind"),
                &JsValue::from_str("external"),
            );
            let _ = Reflect::set(&object, &JsValue::from_str("url"), &JsValue::from_str(url));
            return object.into();
        }
        ReaderEvent::Asset(path) => ("asset", Some(path.to_string_lossy().into_owned())),
        ReaderEvent::NoAction => ("no_action", None),
    };
    let _ = Reflect::set(
        &object,
        &JsValue::from_str("kind"),
        &JsValue::from_str(kind),
    );
    if let Some(path) = path {
        let _ = Reflect::set(
            &object,
            &JsValue::from_str("path"),
            &JsValue::from_str(path.as_str()),
        );
    }
    object.into()
}

/// Return a message from the Rust module after the browser loads it.
#[wasm_bindgen]
pub fn proof_of_life() -> String {
    "PRS-T1 reader web WASM is alive.".to_owned()
}

const ENTRY_POINT: &str = "index.md";
const GLYPH_CACHE_CAPACITY: usize = 256;

/// The browser uses the same logical surface as the PRS-T1 native reader.
#[wasm_bindgen]
pub fn logical_width() -> u32 {
    T1_VIEWPORT.width
}

#[wasm_bindgen]
pub fn logical_height() -> u32 {
    T1_VIEWPORT.height
}

/// An in-memory resource provider for the browser's current Markdown page.
///
/// The reader still resolves and opens a logical entry-point path, just as the
/// filesystem provider does for the native reader. Browser resource loading
/// can replace this provider in a later simulator issue without changing the
/// shared reader or renderer.
#[derive(Clone, Debug)]
struct BrowserResources {
    source: String,
}

impl BrowserResources {
    fn entry_point() -> &'static Path {
        Path::new(ENTRY_POINT)
    }

    fn unsupported(path: &Path) -> ResourceError {
        ResourceError::new(format!(
            "browser simulator has no resource at {}",
            path.display()
        ))
    }
}

impl ResourceProvider for BrowserResources {
    fn document_path(&self) -> &Path {
        Self::entry_point()
    }

    fn entry_point(&self) -> &Path {
        Self::entry_point()
    }

    fn read_text(&self, path: &Path) -> Result<String, ResourceError> {
        if path == Self::entry_point() {
            Ok(self.source.clone())
        } else {
            Err(Self::unsupported(path))
        }
    }

    fn read_binary(&self, path: &Path) -> Result<Vec<u8>, ResourceError> {
        Err(Self::unsupported(path))
    }

    fn resolve_reference(&self, reference: &str) -> Result<ResourceTarget, ResourceError> {
        resolve_reference(reference)
    }

    fn resolve_reference_from(
        &self,
        _containing_document: &Path,
        reference: &str,
    ) -> Result<ResourceTarget, ResourceError> {
        resolve_reference(reference)
    }
}

fn resolve_reference(reference: &str) -> Result<ResourceTarget, ResourceError> {
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return Ok(ResourceTarget::External(reference.to_owned()));
    }

    let (path, anchor) = reference.split_once('#').unwrap_or((reference, ""));
    if path.is_empty() {
        return Ok(ResourceTarget::Anchor(anchor.to_owned()));
    }

    let path = PathBuf::from(path);
    if anchor.is_empty() {
        Ok(ResourceTarget::Document(path))
    } else {
        Ok(ResourceTarget::DocumentAnchor {
            document: path,
            anchor: anchor.to_owned(),
        })
    }
}

/// A tightly packed RGBA framebuffer used as the handoff to JavaScript.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RgbaFramebuffer {
    size: Size,
    pixels: Vec<u8>,
}

impl RgbaFramebuffer {
    fn new(size: Size) -> Self {
        let pixel_count = (size.width as usize)
            .checked_mul(size.height as usize)
            .and_then(|count| count.checked_mul(4))
            .expect("browser framebuffer dimensions must fit in memory");
        let mut framebuffer = Self {
            size,
            pixels: vec![0; pixel_count],
        };
        framebuffer.clear(Rgb888::WHITE).expect("framebuffer clear");
        framebuffer
    }

    fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    fn pixel_index(&self, point: Point) -> Option<usize> {
        let x = u32::try_from(point.x).ok()?;
        let y = u32::try_from(point.y).ok()?;
        if x >= self.size.width || y >= self.size.height {
            return None;
        }
        let offset = (y as usize)
            .checked_mul(self.size.width as usize)?
            .checked_add(x as usize)?
            .checked_mul(4)?;
        Some(offset)
    }
}

impl OriginDimensions for RgbaFramebuffer {
    fn size(&self) -> Size {
        self.size
    }
}

impl DrawTarget for RgbaFramebuffer {
    type Color = Rgb888;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            let Some(index) = self.pixel_index(point) else {
                continue;
            };
            self.pixels[index] = color.r();
            self.pixels[index + 1] = color.g();
            self.pixels[index + 2] = color.b();
            self.pixels[index + 3] = u8::MAX;
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        for pixel in self.pixels.chunks_exact_mut(4) {
            pixel[0] = color.r();
            pixel[1] = color.g();
            pixel[2] = color.b();
            pixel[3] = u8::MAX;
        }
        Ok(())
    }
}

/// A browser-owned reader that delegates parsing, layout, pagination, and
/// rasterization to the shared `prs-markdown` implementation.
#[wasm_bindgen]
pub struct ReaderSimulator {
    reader: Reader<BrowserResources, FontdueTextEngine, ComrakParser>,
    renderer: EmbeddedGraphicsRenderer<FontdueTextEngine>,
    framebuffer: RgbaFramebuffer,
}

#[wasm_bindgen]
impl ReaderSimulator {
    #[wasm_bindgen(constructor)]
    pub fn new(markdown: String) -> Result<ReaderSimulator, JsValue> {
        let fonts = FontConfig::from_faces_with_monospace(
            notosans::REGULAR_TTF,
            notosans::BOLD_TTF,
            notosans::ITALIC_TTF,
            notosans::BOLD_ITALIC_TTF,
            notosans::REGULAR_TTF,
            notosans::BOLD_TTF,
            notosans::ITALIC_TTF,
            notosans::BOLD_ITALIC_TTF,
        );
        let engine = FontdueTextEngine::new(fonts, GLYPH_CACHE_CAPACITY)
            .map_err(|error| JsValue::from_str(&format!("load browser reader fonts: {error}")))?;
        let provider = BrowserResources { source: markdown };
        let mut reader = Reader::with_components(
            provider,
            ComrakParser::default(),
            engine.clone(),
            ReaderStyle::default(),
            T1_VIEWPORT,
        );
        reader
            .open()
            .map_err(|error| JsValue::from_str(&format!("open browser reader: {error}")))?;

        Ok(Self {
            reader,
            renderer: EmbeddedGraphicsRenderer::new(engine),
            framebuffer: RgbaFramebuffer::new(Size::new(T1_VIEWPORT.width, T1_VIEWPORT.height)),
        })
    }

    /// Render the current page and return tightly packed RGBA bytes.
    ///
    /// The returned array always contains `600 * 800 * 4` bytes. JavaScript
    /// may copy it into an `ImageData` object; CSS can scale the canvas without
    /// changing these logical dimensions.
    pub fn render_frame(&mut self) -> Result<Vec<u8>, JsValue> {
        self.framebuffer
            .clear(Rgb888::WHITE)
            .expect("framebuffer clear cannot fail");
        self.reader
            .render_current_page(&mut self.renderer, &mut self.framebuffer)
            .map_err(|error| JsValue::from_str(&format!("render browser reader: {error}")))?;
        Ok(self.framebuffer.pixels().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MARKDOWN: &str = "# Browser reader\n\nThe shared renderer owns this page.\n";

    #[test]
    fn framebuffer_writes_rgba_pixels_and_discards_out_of_bounds_pixels() {
        let mut framebuffer = RgbaFramebuffer::new(Size::new(1, 1));
        framebuffer
            .draw_iter([
                Pixel(Point::new(-1, 0), Rgb888::RED),
                Pixel(Point::zero(), Rgb888::new(12, 34, 56)),
            ])
            .expect("framebuffer draw cannot fail");

        assert_eq!(framebuffer.pixels(), &[12, 34, 56, u8::MAX]);
    }

    #[test]
    fn simulator_renders_a_deterministic_600_by_800_frame() {
        let mut first = ReaderSimulator::new(SAMPLE_MARKDOWN.to_owned()).expect("open reader");
        let mut second = ReaderSimulator::new(SAMPLE_MARKDOWN.to_owned()).expect("open reader");

        let first_frame = first.render_frame().expect("render reader");
        let second_frame = second.render_frame().expect("render reader");

        assert_eq!(logical_width(), 600);
        assert_eq!(logical_height(), 800);
        assert_eq!(first_frame.len(), 600 * 800 * 4);
        assert_eq!(first_frame, second_frame);
        assert!(first_frame
            .chunks_exact(4)
            .any(|pixel| pixel != [u8::MAX; 4]));
    }
}
