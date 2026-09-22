use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{OriginDimensions, Point, Size};
use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::prelude::Pixel;
use js_sys::{Array, Object, Reflect, Uint8Array};
use prs_markdown::navigation::{DocumentId, DocumentLocation, NavigationTarget};
use prs_markdown::parse::ComrakParser;
use prs_markdown::reader::ReaderEvent;
use prs_markdown::render::EmbeddedGraphicsRenderer;
use prs_markdown::typography::{FontConfig, FontdueTextEngine};
use prs_markdown::{
    BrowserResourceProvider as DirectoryResourceProvider, Reader, ReaderController,
    ReaderControllerError, ReaderLayout, ReaderStyle, ResourceProvider, ResourceTarget,
    T1_VIEWPORT,
};
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
    controller: ReaderController<P, FontdueTextEngine, ComrakParser>,
    renderer: EmbeddedGraphicsRenderer<FontdueTextEngine>,
    framebuffer: RgbaFramebuffer,
    feedback: String,
}

impl<P> ReaderSurface<P>
where
    P: ResourceProvider,
{
    fn new(provider: P) -> Result<Self, String> {
        Self::new_with_layout(provider, ReaderLayout::content(T1_VIEWPORT))
    }

    fn new_with_layout(provider: P, layout: ReaderLayout) -> Result<Self, String> {
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
        let mut reader = Reader::with_layout(
            provider,
            ComrakParser::default(),
            engine.clone(),
            layout,
            ReaderStyle::default(),
        );
        reader
            .open()
            .map_err(|error| format!("open browser reader: {error}"))?;

        Ok(Self {
            controller: ReaderController::new(reader),
            renderer: EmbeddedGraphicsRenderer::new(engine),
            framebuffer: RgbaFramebuffer::new(Size::new(
                layout.display_viewport.width,
                layout.display_viewport.height,
            )),
            feedback: "Ready. Use a link, a control, or a keyboard shortcut.".to_owned(),
        })
    }

    fn render_frame(&mut self) -> Result<Vec<u8>, String> {
        self.framebuffer
            .clear(Rgb888::WHITE)
            .expect("framebuffer clear cannot fail");
        self.controller
            .render_current_page_with_overlay(
                &mut self.renderer,
                &mut self.framebuffer,
                Point::new(
                    0,
                    self.controller.reader().reader_layout().content_top() as i32,
                ),
            )
            .map_err(|error| format!("render browser reader: {error}"))?;
        Ok(self.framebuffer.pixels().to_vec())
    }

    fn pointer_up(&mut self, x: f64, y: f64) -> String {
        let layout = self.controller.reader().reader_layout();
        let display_viewport = layout.display_viewport;
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.0
            || x >= f64::from(display_viewport.width)
            || y < 0.0
            || y >= f64::from(display_viewport.height)
            || y < f64::from(layout.content_top())
        {
            self.feedback = "No reader action.".to_owned();
            return self.feedback.clone();
        }

        let point = Point::new(
            x.floor() as i32,
            y.floor() as i32 - layout.content_top() as i32,
        );
        if !layout.effective_viewport().contains(point) {
            self.feedback = "No reader action.".to_owned();
            return self.feedback.clone();
        }
        let result = self.controller.activate_at_or_page_turn(point);
        self.apply_result(result)
    }

    fn previous(&mut self) -> String {
        let result = self.controller.previous_page();
        self.apply_result(result)
    }

    fn next(&mut self) -> String {
        let result = self.controller.next_page();
        self.apply_result(result)
    }

    fn back(&mut self) -> String {
        let result = self.controller.back();
        self.apply_result(result)
    }

    fn home(&mut self) -> String {
        let entry_point = self
            .controller
            .reader()
            .provider()
            .entry_point()
            .display()
            .to_string();
        let result = self
            .controller
            .activate(NavigationTarget::Location(DocumentLocation::new(
                DocumentId::from(entry_point),
                None,
            )));
        self.apply_result(result)
    }

    fn current_document(&self) -> String {
        self.controller
            .reader()
            .current_location()
            .map(|location| location.document.as_ref().to_owned())
            .unwrap_or_default()
    }

    fn page_count(&self) -> u32 {
        self.controller
            .reader()
            .page_count()
            .try_into()
            .unwrap_or(u32::MAX)
    }

    fn current_page(&self) -> u32 {
        self.controller
            .reader()
            .current_page_number()
            .unwrap_or_default() as u32
    }

    fn feedback(&self) -> String {
        self.feedback.clone()
    }

    fn apply_result(&mut self, result: Result<ReaderEvent, ReaderControllerError>) -> String {
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
            .controller
            .reader()
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
            .controller
            .reader()
            .current_location()
            .map(|location| PathBuf::from(location.document.as_ref()))
            .ok_or_else(|| structured_error("reader", "no document is open"))?;
        let target = self
            .surface
            .controller
            .reader()
            .provider()
            .resolve_reference_from(&containing_document, reference)
            .map_err(|error| structured_error("resource", error))?;
        Ok(target_value(&target))
    }

    /// Follow a document, anchor, asset, or external URL reference.
    pub fn follow_reference(&mut self, reference: &str) -> Result<JsValue, JsValue> {
        let event = self
            .surface
            .controller
            .follow_reference(reference)
            .map_err(|error| structured_reader_error("navigate", error))?;
        Ok(event_value(&event))
    }

    /// Read a selected-directory asset through the same root-relative checks
    /// used by Markdown image loading.
    pub fn read_asset(&self, path: &str) -> Result<Uint8Array, JsValue> {
        let bytes = self
            .surface
            .controller
            .reader()
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

fn structured_reader_error(kind: &str, error: impl std::fmt::Display) -> JsValue {
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

const ENTRY_POINT: &str = "README.md";
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

/// Return the checked-in demo through the same provider used by a selected
/// browser directory. Keeping this fixture in the Rust test path makes the
/// host checks exercise real Markdown, image decoding, and link resolution.
fn demo_provider() -> DirectoryResourceProvider {
    DirectoryResourceProvider::new(
        ENTRY_POINT,
        [
            (
                PathBuf::from("README.md"),
                include_bytes!("../demo/README.md").to_vec(),
            ),
            (
                PathBuf::from("guide/chapter.md"),
                include_bytes!("../demo/guide/chapter.md").to_vec(),
            ),
            (
                PathBuf::from("guide/notes.md"),
                include_bytes!("../demo/guide/notes.md").to_vec(),
            ),
            (
                PathBuf::from("assets/observatory.png"),
                include_bytes!("../demo/assets/observatory.png").to_vec(),
            ),
            (
                PathBuf::from("assets/detail.png"),
                include_bytes!("../demo/assets/detail.png").to_vec(),
            ),
        ],
    )
    .expect("checked-in browser demo fixture is valid")
}

/// A fixture-backed reader useful for host checks and browser smoke validation.
/// The assembled page uses [`BrowserReader`] after either loading this same
/// fixture or taking a user-selected directory snapshot.
#[wasm_bindgen]
pub struct ReaderSimulator {
    surface: ReaderSurface<DirectoryResourceProvider>,
}

#[wasm_bindgen]
impl ReaderSimulator {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<ReaderSimulator, JsValue> {
        let surface =
            ReaderSurface::new(demo_provider()).map_err(|error| JsValue::from_str(&error))?;
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
        assert!(app.surface.controller.reader().page_count() > 1);
        let link = app
            .surface
            .controller
            .reader()
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
                .controller
                .reader()
                .current_location()
                .map(|location| location.document.as_ref()),
            Some("guide/chapter.md")
        );
        assert!(app.surface.controller.reader().can_go_back());

        app.back();
        assert_eq!(
            app.surface
                .controller
                .reader()
                .current_location()
                .map(|location| location.document.as_ref()),
            Some(ENTRY_POINT)
        );
        app.next();
        assert_eq!(
            app.surface.controller.reader().current_page_index(),
            Some(1)
        );
        app.previous();
        assert_eq!(
            app.surface.controller.reader().current_page_index(),
            Some(0)
        );
    }

    #[test]
    fn browser_external_link_overlay_renders_and_next_input_clears_it() {
        let mut app = ReaderSimulator::new().expect("demo reader opens");
        let external_link = app
            .surface
            .controller
            .reader()
            .current_page()
            .expect("entry page")
            .hit_regions
            .iter()
            .find(|region| matches!(region.target, NavigationTarget::External(_)))
            .expect("entry page external link");

        let feedback = app.pointer_up(
            f64::from(external_link.bounds.top_left.x + 1),
            f64::from(external_link.bounds.top_left.y + 1),
        );
        assert!(feedback.starts_with("External link: https://"));
        assert!(app.surface.controller.external_link_overlay().is_some());

        let overlay_frame = app.render_frame().expect("render external-link overlay");
        let region = prs_markdown::external_link_overlay_region(600, 800);
        let dark_pixels = overlay_frame
            .chunks_exact(4)
            .enumerate()
            .filter(|(index, pixel)| {
                let x = (*index % 600) as i32;
                let y = (*index / 600) as i32;
                region.contains(embedded_graphics::geometry::Point::new(x, y))
                    && pixel[..3].iter().any(|channel| *channel < u8::MAX)
            })
            .count();
        assert!(
            dark_pixels > 0,
            "overlay should add visible text or QR pixels"
        );

        app.next();
        assert!(app.surface.controller.external_link_overlay().is_none());
        let cleared_frame = app.render_frame().expect("render dismissed overlay");
        assert_ne!(overlay_frame, cleared_frame);
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

    #[test]
    fn demo_fixture_covers_directory_rendering_links_images_and_history() {
        let provider = demo_provider();
        assert_eq!(provider.entry_point(), Path::new(ENTRY_POINT));
        assert!(
            provider
                .read_binary(Path::new("assets/observatory.png"))
                .expect("demo image is readable")
                .len()
                > 100
        );
        assert!(matches!(
            provider
                .resolve_reference_from(Path::new("guide/chapter.md"), "../assets/observatory.png")
                .expect("relative image resolves"),
            ResourceTarget::Asset(path) if path == Path::new("assets/observatory.png")
        ));
        assert!(provider.read_binary(Path::new("assets/detail.png")).is_ok());

        let mut surface = ReaderSurface::new(provider).expect("open demo fixture");
        assert_eq!(surface.current_document(), ENTRY_POINT);
        assert!(surface.page_count() > 1);
        assert!(
            surface
                .controller
                .reader()
                .cache_stats()
                .image_retained_bytes
                > 0
        );

        let frame = surface.render_frame().expect("render demo fixture");
        assert_eq!(frame.len(), 600 * 800 * 4);

        let chapter_link = surface
            .controller
            .reader()
            .current_page()
            .expect("demo entry page")
            .hit_regions
            .first()
            .expect("demo chapter link")
            .bounds
            .top_left;
        assert!(matches!(
            surface.pointer_up(
                f64::from(chapter_link.x + 1),
                f64::from(chapter_link.y + 1)
            ),
            message if message == "Followed the reader link."
        ));
        assert_eq!(surface.current_document(), "guide/chapter.md");

        assert!(matches!(
            surface
                .controller
                .follow_reference("https://example.com/prs-t1-demo"),
            Ok(ReaderEvent::ExternalUrl(url)) if url == "https://example.com/prs-t1-demo"
        ));
        surface.home();
        assert_eq!(surface.current_document(), ENTRY_POINT);
        surface.next();
        assert_eq!(surface.controller.reader().current_page_index(), Some(1));
        surface.previous();
        assert_eq!(surface.controller.reader().current_page_index(), Some(0));
        surface.back();
        assert_eq!(surface.current_document(), "guide/chapter.md");
    }
}
