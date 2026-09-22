use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{OriginDimensions, Point, Size};
use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::prelude::Pixel;
use js_sys::{Array, Object, Reflect, Uint8Array};
use prs_markdown::navigation::{DocumentId, DocumentLocation, NavigationTarget};
use prs_markdown::parse::ComrakParser;
use prs_markdown::reader::{ReaderError, ReaderEvent};
use prs_markdown::render::EmbeddedGraphicsRenderer;
use prs_markdown::resources::ResourceError;
use prs_markdown::typography::{FontConfig, FontdueTextEngine};
use prs_markdown::{
    BrowserResourceProvider as DirectoryResourceProvider, Reader, ReaderStyle, ResourceProvider,
    ResourceTarget, T1_VIEWPORT,
};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// The rendered reader surface shared by the demo and directory-backed
/// sessions. The browser only supplies a resource provider; parsing, layout,
/// navigation, hit testing, and rasterization stay in the shared Rust reader.
struct ReaderSurface<P>
where
    P: ResourceProvider,
{
    reader: Reader<P, FontdueTextEngine, ComrakParser>,
    renderer: EmbeddedGraphicsRenderer<FontdueTextEngine>,
    framebuffer: RgbaFramebuffer,
    feedback: String,
}

impl<P> ReaderSurface<P>
where
    P: ResourceProvider,
{
    fn new(provider: P) -> Result<Self, String> {
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
            .map_err(|error| format!("load browser reader fonts: {error}"))?;
        let mut reader = Reader::with_components(
            provider,
            ComrakParser::default(),
            engine.clone(),
            ReaderStyle::default(),
            T1_VIEWPORT,
        );
        reader
            .open()
            .map_err(|error| format!("open browser reader: {error}"))?;

        Ok(Self {
            reader,
            renderer: EmbeddedGraphicsRenderer::new(engine),
            framebuffer: RgbaFramebuffer::new(Size::new(T1_VIEWPORT.width, T1_VIEWPORT.height)),
            feedback: "Ready. Use a link, a control, or a keyboard shortcut.".to_owned(),
        })
    }

    fn render_frame(&mut self) -> Result<Vec<u8>, String> {
        self.framebuffer
            .clear(Rgb888::WHITE)
            .expect("framebuffer clear cannot fail");
        self.reader
            .render_current_page(&mut self.renderer, &mut self.framebuffer)
            .map_err(|error| format!("render browser reader: {error}"))?;
        Ok(self.framebuffer.pixels().to_vec())
    }

    fn pointer_up(&mut self, x: f64, y: f64) -> String {
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.0
            || x >= f64::from(T1_VIEWPORT.width)
            || y < 0.0
            || y >= f64::from(T1_VIEWPORT.height)
        {
            self.feedback = "No reader action.".to_owned();
            return self.feedback.clone();
        }

        let point = Point::new(x.floor() as i32, y.floor() as i32);
        let result = self.reader.activate_at(point);
        let result = match result {
            Ok(ReaderEvent::NoAction) => {
                if point.x >= self.reader.viewport().width as i32 / 2 {
                    self.reader.next_page_event()
                } else {
                    self.reader.previous_page_event()
                }
            }
            result => result,
        };
        self.apply_result(result)
    }

    fn previous(&mut self) -> String {
        let result = self.reader.previous_page_event();
        self.apply_result(result)
    }

    fn next(&mut self) -> String {
        let result = self.reader.next_page_event();
        self.apply_result(result)
    }

    fn back(&mut self) -> String {
        let result = self.reader.back_event();
        self.apply_result(result)
    }

    fn home(&mut self) -> String {
        let entry_point = self.reader.provider().entry_point().display().to_string();
        let result = self
            .reader
            .activate(NavigationTarget::Location(DocumentLocation::new(
                DocumentId::from(entry_point),
                None,
            )));
        self.apply_result(result)
    }

    fn current_document(&self) -> String {
        self.reader
            .current_location()
            .map(|location| location.document.as_ref().to_owned())
            .unwrap_or_default()
    }

    fn page_count(&self) -> u32 {
        self.reader.page_count().try_into().unwrap_or(u32::MAX)
    }

    fn current_page(&self) -> u32 {
        self.reader.current_page_number().unwrap_or_default() as u32
    }

    fn feedback(&self) -> String {
        self.feedback.clone()
    }

    fn apply_result(&mut self, result: Result<ReaderEvent, ReaderError>) -> String {
        self.feedback = match result {
            Ok(ReaderEvent::ExternalUrl(url)) => format!("External link: {url}"),
            Ok(ReaderEvent::Asset(path)) => format!("Asset link: {}", path.display()),
            Ok(ReaderEvent::NoAction) => "No reader action.".to_owned(),
            Ok(ReaderEvent::Opened { .. }) => "Opened the entry document.".to_owned(),
            Ok(ReaderEvent::PageChanged { .. }) => "Page changed.".to_owned(),
            Ok(ReaderEvent::Navigated { .. }) => "Followed the reader link.".to_owned(),
            Ok(ReaderEvent::Back { .. }) => "Returned through reader history.".to_owned(),
            Ok(ReaderEvent::Forward { .. }) => "Moved forward through reader history.".to_owned(),
            Err(error) => format!("Reader error: {error}"),
        };
        self.feedback.clone()
    }
}

/// A reader backed by a snapshot of the directory selected in the browser.
///
/// The File System Access API is asynchronous, so JavaScript creates the
/// snapshot before calling [`load_directory`]. Once constructed, the shared
/// reader uses the same synchronous `ResourceProvider` boundary as native
/// readers and renders through the same WASM framebuffer as the demo reader.
#[wasm_bindgen]
pub struct BrowserReader {
    surface: ReaderSurface<DirectoryResourceProvider>,
}

/// Build and open a reader from the files returned by `directory-library.js`.
///
/// Each array item must be an object with a relative `path` string and a
/// `Uint8Array` of `bytes`. The entry point defaults to `README.md` in the
/// browser UI, but callers may supply another root-relative Markdown path.
#[wasm_bindgen]
pub fn load_directory(files: Array, entry_point: String) -> Result<BrowserReader, JsValue> {
    let files = parse_files(files)?;
    let provider = DirectoryResourceProvider::new(Path::new(&entry_point), files)
        .map_err(|error| structured_error("resource", error))?;
    let surface = ReaderSurface::new(provider).map_err(|error| structured_error("open", error))?;
    Ok(BrowserReader { surface })
}

#[wasm_bindgen]
impl BrowserReader {
    pub fn entry_point(&self) -> String {
        self.surface
            .reader
            .provider()
            .entry_point()
            .display()
            .to_string()
    }

    pub fn current_document(&self) -> String {
        self.surface.current_document()
    }

    pub fn page_count(&self) -> u32 {
        self.surface.page_count()
    }

    pub fn current_page(&self) -> u32 {
        self.surface.current_page()
    }

    pub fn feedback(&self) -> String {
        self.surface.feedback()
    }

    pub fn render_frame(&mut self) -> Result<Vec<u8>, JsValue> {
        self.surface
            .render_frame()
            .map_err(|error| JsValue::from_str(&error))
    }

    pub fn pointer_up(&mut self, x: f64, y: f64) -> String {
        self.surface.pointer_up(x, y)
    }

    pub fn previous(&mut self) -> String {
        self.surface.previous()
    }

    pub fn next(&mut self) -> String {
        self.surface.next()
    }

    pub fn back(&mut self) -> String {
        self.surface.back()
    }

    pub fn home(&mut self) -> String {
        self.surface.home()
    }

    /// Resolve a local document or asset through the selected directory.
    pub fn resolve_reference(&self, reference: &str) -> Result<JsValue, JsValue> {
        let containing_document = self
            .surface
            .reader
            .current_location()
            .map(|location| PathBuf::from(location.document.as_ref()))
            .ok_or_else(|| structured_error("reader", "no document is open"))?;
        let target = self
            .surface
            .reader
            .provider()
            .resolve_reference_from(&containing_document, reference)
            .map_err(|error| structured_error("resource", error))?;
        Ok(target_value(&target))
    }

    /// Follow a document, anchor, asset, or external URL reference.
    pub fn follow_reference(&mut self, reference: &str) -> Result<JsValue, JsValue> {
        let event = self
            .surface
            .reader
            .follow_reference(reference)
            .map_err(|error| structured_reader_error("navigate", error))?;
        Ok(event_value(&event))
    }

    /// Read a selected-directory asset through the same root-relative checks
    /// used by Markdown image loading.
    pub fn read_asset(&self, path: &str) -> Result<Uint8Array, JsValue> {
        let bytes = self
            .surface
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

/// A small in-memory provider keeps the browser simulator focused on the same
/// reader and navigation code as the native application.
#[derive(Clone, Debug)]
struct DemoResourceProvider {
    documents: BTreeMap<String, String>,
    entry_point: PathBuf,
}

impl DemoResourceProvider {
    fn demo() -> Self {
        let mut documents = BTreeMap::new();
        documents.insert(
            ENTRY_POINT.to_owned(),
            r#"# PRS-T1 browser reader

This page uses the shared Markdown reader. The browser supplies input events;
Rust owns hit testing, page turns, and document navigation.

Try the [linked chapter](chapter.md#interactive) or open an [external link](https://example.com/prs-t1).

## Input

Use the Previous and Next controls, the Home and Back controls, or the keyboard
shortcuts. A blank tap on the right half advances one page. A blank tap on the
left half goes back one page.

The canvas is always a logical 600 by 800 reader surface. CSS can scale it
without changing the coordinates used by the reader.

## More reading

This extra content gives the simulator several pages so the Previous and Next
controls exercise real shared pagination rather than only reporting a boundary.

The native reader and the browser reader use the same page-space coordinates.
Only the platform event adapter changes between the two environments.

Reader state remains in Rust while the browser redraws the returned framebuffer.
The JavaScript layer does not inspect links or decide where a page turn goes.

The logical surface stays stable when the surrounding page is narrow or wide.
Try resizing the browser window and activating the same visible link again.

This paragraph continues the demo document so the page boundary is easy to
reach with a pointer, keyboard shortcut, or visible control button.

The first page contains the link targets. Later pages contain ordinary text so
blank-area taps can be used to verify the page-turn behavior.

The reader does not use the browser location bar as a navigation state. The
current document and page remain owned by the Rust reader session.

The controls call the same page-event methods that a native input adapter uses.
They do not maintain a second browser-side page counter.

The Back action is history-aware. It restores the document and reading cursor
that the shared reader saved when a link was activated.

The Home action follows the entry location through the shared navigation path.
It is not a special browser-only reset.

Resize the page again after reading this section. The logical coordinates do
not change when the canvas is rendered at a different CSS width.
"#
            .to_owned(),
        );
        documents.insert(
            "chapter.md".to_owned(),
            r#"# Linked chapter

## Interactive

This page was opened through a semantic Markdown link. Use Back to return to
the exact page and reading position where the link was activated.

The Home control returns to the entry document through the shared reader
history. The browser does not reimplement these navigation rules.

[Return to the entry page](index.md)
"#
            .to_owned(),
        );
        Self {
            documents,
            entry_point: PathBuf::from(ENTRY_POINT),
        }
    }

    fn key(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    fn normalize_path(path: &str) -> String {
        let mut parts = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                part => parts.push(part),
            }
        }
        parts.join("/")
    }

    fn is_external(reference: &str) -> bool {
        reference.starts_with("//")
            || reference.find(':').is_some_and(|colon| {
                colon > 0
                    && reference[..colon]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            })
    }
}

impl ResourceProvider for DemoResourceProvider {
    fn document_path(&self) -> &Path {
        &self.entry_point
    }

    fn entry_point(&self) -> &Path {
        &self.entry_point
    }

    fn read_text(&self, path: &Path) -> Result<String, ResourceError> {
        let key = Self::key(path);
        self.documents
            .get(&key)
            .cloned()
            .ok_or_else(|| ResourceError::new(format!("browser demo document not found: {key}")))
    }

    fn read_binary(&self, path: &Path) -> Result<Vec<u8>, ResourceError> {
        Err(ResourceError::new(format!(
            "browser demo has no binary resource: {}",
            path.display()
        )))
    }

    fn resolve_reference(&self, reference: &str) -> Result<ResourceTarget, ResourceError> {
        self.resolve_reference_from(&self.entry_point, reference)
    }

    fn resolve_reference_from(
        &self,
        containing_document: &Path,
        reference: &str,
    ) -> Result<ResourceTarget, ResourceError> {
        if Self::is_external(reference) {
            return Ok(ResourceTarget::External(reference.to_owned()));
        }

        let (path, anchor) = reference
            .split_once('#')
            .map_or((reference, None), |(path, anchor)| (path, Some(anchor)));
        if path.is_empty() {
            return anchor.map_or_else(
                || Ok(ResourceTarget::Document(containing_document.to_owned())),
                |anchor| Ok(ResourceTarget::Anchor(anchor.to_owned())),
            );
        }

        let directory = containing_document
            .parent()
            .map(|parent| parent.to_string_lossy())
            .filter(|parent| !parent.is_empty())
            .map_or_else(String::new, |parent| parent.into_owned());
        let joined = if directory.is_empty() {
            path.to_owned()
        } else {
            format!("{directory}/{path}")
        };
        let resolved = PathBuf::from(Self::normalize_path(&joined));
        let markdown = resolved.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        });

        if markdown {
            Ok(match anchor {
                Some(anchor) => ResourceTarget::DocumentAnchor {
                    document: resolved,
                    anchor: anchor.to_owned(),
                },
                None => ResourceTarget::Document(resolved),
            })
        } else {
            Ok(ResourceTarget::Asset(resolved))
        }
    }
}

/// A browser-owned reader that delegates parsing, layout, navigation, hit
/// testing, and rasterization to the shared prs-markdown implementation.
#[wasm_bindgen]
pub struct ReaderSimulator {
    surface: ReaderSurface<DemoResourceProvider>,
}

#[wasm_bindgen]
impl ReaderSimulator {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<ReaderSimulator, JsValue> {
        let surface = ReaderSurface::new(DemoResourceProvider::demo())
            .map_err(|error| JsValue::from_str(&error))?;
        Ok(Self { surface })
    }

    pub fn current_document(&self) -> String {
        self.surface.current_document()
    }

    pub fn current_page(&self) -> u32 {
        self.surface.current_page()
    }

    pub fn page_count(&self) -> u32 {
        self.surface.page_count()
    }

    pub fn feedback(&self) -> String {
        self.surface.feedback()
    }

    pub fn render_frame(&mut self) -> Result<Vec<u8>, JsValue> {
        self.surface
            .render_frame()
            .map_err(|error| JsValue::from_str(&error))
    }

    pub fn pointer_up(&mut self, x: f64, y: f64) -> String {
        self.surface.pointer_up(x, y)
    }

    pub fn previous(&mut self) -> String {
        self.surface.previous()
    }

    pub fn next(&mut self) -> String {
        self.surface.next()
    }

    pub fn back(&mut self) -> String {
        self.surface.back()
    }

    pub fn home(&mut self) -> String {
        self.surface.home()
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut first = ReaderSimulator::new().expect("open reader");
        let mut second = ReaderSimulator::new().expect("open reader");

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

    #[test]
    fn browser_reader_exposes_shared_links_and_history() {
        let mut app = ReaderSimulator::new().expect("demo reader opens");
        assert!(app.surface.reader.page_count() > 1);
        let link = app
            .surface
            .reader
            .current_page()
            .expect("entry page")
            .hit_regions
            .first()
            .expect("entry page link")
            .bounds
            .top_left;

        app.pointer_up(f64::from(link.x + 1), f64::from(link.y + 1));
        assert_eq!(
            app.surface
                .reader
                .current_location()
                .map(|location| location.document.as_ref()),
            Some("chapter.md")
        );
        assert!(app.surface.reader.can_go_back());

        app.back();
        assert_eq!(
            app.surface
                .reader
                .current_location()
                .map(|location| location.document.as_ref()),
            Some(ENTRY_POINT)
        );
        app.next();
        assert_eq!(app.surface.reader.current_page_index(), Some(1));
        app.previous();
        assert_eq!(app.surface.reader.current_page_index(), Some(0));
    }

    #[test]
    fn directory_reader_renders_the_selected_entry_point() {
        let provider = DirectoryResourceProvider::new(
            Path::new("README.md"),
            vec![(
                PathBuf::from("README.md"),
                b"# Selected directory\n\nThis is rendered by the live directory reader.\n"
                    .to_vec(),
            )],
        )
        .expect("create directory provider");
        let mut surface = ReaderSurface::new(provider).expect("open directory reader");

        assert_eq!(surface.current_document(), "README.md");
        let frame = surface.render_frame().expect("render directory reader");
        assert_eq!(frame.len(), 600 * 800 * 4);
        assert!(frame.chunks_exact(4).any(|pixel| pixel != [u8::MAX; 4]));
    }
}
