use crate::framebuffer::{DisplayCanvas, DisplayRegion, NativeDisplay, WaveformMode};
use crate::qr::QrMatrix;
use embedded_graphics::mono_font::{
    ascii::{FONT_10X20, FONT_8X13, FONT_8X13_BOLD},
    MonoTextStyle,
};
use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};
use std::convert::Infallible;
#[cfg(test)]
use std::io::{self, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

const BLACK: u16 = 0x0000;
const WHITE: u16 = 0xffff;

const DISPLAY_TEST_HEADER_HEIGHT: usize = 52;
const DISPLAY_TEST_GRID_TOP: usize = 78;
const DISPLAY_TEST_GRID_TILE_HEIGHT: usize = 86;
const DISPLAY_TEST_GRID_GAP: usize = 8;
const DISPLAY_TEST_GRADIENT_TOP: usize = 395;
const DISPLAY_TEST_GRADIENT_HEIGHT: usize = 36;
const DISPLAY_TEST_PANELS_TOP: usize = 464;
const DISPLAY_TEST_PANELS_HEIGHT: usize = 120;
const DISPLAY_TEST_RAMP_TOP: usize = 620;
const DISPLAY_TEST_RAMP_HEIGHT: usize = 48;

pub const STATUS_BAR_HEIGHT: usize = 48;
pub const CONTENT_TOP: usize = 76;
pub const CONTENT_LINE_STEP: usize = 25;
pub const DETAILS_LINE_STEP: usize = 21;
pub const DETAILS_ACTION_MARGIN: usize = 24;
pub const DETAILS_DEBUG_TOP: usize = 106;
pub const DETAILS_DEBUG_HEIGHT: usize = 32;
pub const DETAILS_ACTION_HEADER_TOP: usize = 520;
pub const DETAILS_ACTION_TOP: usize = 548;
pub const DETAILS_ACTION_HEIGHT: usize = 36;
pub const DETAILS_ACTION_GAP: usize = 4;
pub const DETAILS_SYNC_TOP: usize = DETAILS_ACTION_TOP;
pub const DETAILS_RETURN_ENTRY_TOP: usize =
    DETAILS_SYNC_TOP + DETAILS_ACTION_HEIGHT + DETAILS_ACTION_GAP;
pub const DETAILS_DISPLAY_TEST_TOP: usize =
    DETAILS_RETURN_ENTRY_TOP + DETAILS_ACTION_HEIGHT + DETAILS_ACTION_GAP;
pub const DETAILS_REBOOT_TOP: usize =
    DETAILS_DISPLAY_TEST_TOP + DETAILS_ACTION_HEIGHT + DETAILS_ACTION_GAP;
pub const DETAILS_POWER_OFF_TOP: usize =
    DETAILS_REBOOT_TOP + DETAILS_ACTION_HEIGHT + DETAILS_ACTION_GAP;
pub const DETAILS_BACK_TOP: usize =
    DETAILS_POWER_OFF_TOP + DETAILS_ACTION_HEIGHT + DETAILS_ACTION_GAP;
pub const SCREEN_WIDTH: usize = 600;
pub const SCREEN_HEIGHT: usize = 800;

const STATUS_BAR_SIDE_MARGIN: usize = 16;
const STATUS_BAR_CLOCK_WIDTH: usize = 96;
const STATUS_ICON_GAP: usize = 4;
const STATUS_MODE_WIDTH: usize = 96;
const BATTERY_ICON: &[u8] = include_bytes!("../assets/battery-20x20.bin");
const WIFI_ICON: &[u8] = include_bytes!("../assets/wifi-20x20.bin");
const USB_ICON: &[u8] = include_bytes!("../assets/usb-20x20.bin");
const ADB_ICON: &[u8] = include_bytes!("../assets/adb-20x20.bin");
const CLOCK_ICON: &[u8] = include_bytes!("../assets/clock-20x20.bin");

/// The display data needed by the native status bar.
///
/// The values are already formatted for the small native UI. Keeping this
/// model independent of sysfs and process state makes the renderer usable in
/// deterministic host tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusBarViewModel {
    pub battery: String,
    pub wifi: String,
    pub usb: String,
    pub adb: String,
    pub mode: String,
    pub clock: String,
}

impl StatusBarViewModel {
    pub fn from_wire_line(line: &str) -> Self {
        let mut fields = line.split('|');
        Self {
            battery: fields.next().unwrap_or_default().into(),
            wifi: fields.next().unwrap_or_default().into(),
            usb: fields.next().unwrap_or_default().into(),
            adb: fields.next().unwrap_or_default().into(),
            mode: fields.next().unwrap_or_default().into(),
            clock: fields.next().unwrap_or_default().into(),
        }
    }

    pub fn to_wire_line(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.battery, self.wifi, self.usb, self.adb, self.mode, self.clock
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailsRow {
    Section(String),
    Value(String),
    Toggle { label: String, enabled: bool },
}

/// An actionable control on the native Details / Settings page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailsAction {
    SyncNow,
    ReturnToEntryPoint,
    DisplayTest,
    Reboot,
    PowerOff,
    BackToReading,
}

impl DetailsAction {
    pub const ALL: [Self; 6] = [
        Self::SyncNow,
        Self::ReturnToEntryPoint,
        Self::DisplayTest,
        Self::Reboot,
        Self::PowerOff,
        Self::BackToReading,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::SyncNow => "Sync now",
            Self::ReturnToEntryPoint => "Return to entry point",
            Self::DisplayTest => "Display test",
            Self::Reboot => "Reboot",
            Self::PowerOff => "Power off",
            Self::BackToReading => "Back to reading",
        }
    }

    pub const fn top(self) -> usize {
        match self {
            Self::SyncNow => DETAILS_SYNC_TOP,
            Self::ReturnToEntryPoint => DETAILS_RETURN_ENTRY_TOP,
            Self::DisplayTest => DETAILS_DISPLAY_TEST_TOP,
            Self::Reboot => DETAILS_REBOOT_TOP,
            Self::PowerOff => DETAILS_POWER_OFF_TOP,
            Self::BackToReading => DETAILS_BACK_TOP,
        }
    }

    const fn is_destructive(self) -> bool {
        matches!(self, Self::Reboot | Self::PowerOff)
    }
}

/// Return the exact action rectangle used by both rendering and hit testing.
pub fn details_action_region(action: DetailsAction, width: usize, height: usize) -> DisplayRegion {
    let margin = DETAILS_ACTION_MARGIN.min(width / 2);
    DisplayRegion::new(
        margin as u32,
        action.top().min(height) as u32,
        width.saturating_sub(margin.saturating_mul(2)) as u32,
        DETAILS_ACTION_HEIGHT.min(height.saturating_sub(action.top())) as u32,
    )
}

/// Hit-test a Details / Settings action against the same geometry drawn on screen.
pub fn details_action_at(x: i32, y: i32, width: usize, height: usize) -> Option<DetailsAction> {
    if x < 0 || y < 0 {
        return None;
    }
    let x = x as usize;
    let y = y as usize;
    DetailsAction::ALL.into_iter().find(|action| {
        let region = details_action_region(*action, width, height);
        x >= region.left as usize
            && x < region.right() as usize
            && y >= region.top as usize
            && y < region.bottom() as usize
    })
}

/// The native Details / Settings data after status collection and formatting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailsViewModel {
    pub title: String,
    pub rows: Vec<DetailsRow>,
}

impl DetailsViewModel {
    pub fn new(title: impl Into<String>, rows: Vec<DetailsRow>) -> Self {
        Self {
            title: title.into(),
            rows,
        }
    }

    pub fn from_lines(lines: &[String]) -> Self {
        let title = lines
            .first()
            .cloned()
            .unwrap_or_else(|| "Details / Settings".into());
        let rows = lines
            .iter()
            .skip(1)
            .map(|line| {
                if let Some(enabled) = debug_toggle_state(line) {
                    DetailsRow::Toggle {
                        label: "Debug messages".into(),
                        enabled,
                    }
                } else if is_section_heading(line) {
                    DetailsRow::Section(line.clone())
                } else {
                    DetailsRow::Value(line.clone())
                }
            })
            .collect();
        Self { title, rows }
    }

    pub fn to_lines(&self) -> Vec<String> {
        let mut lines = vec![self.title.clone()];
        lines.extend(self.rows.iter().map(|row| match row {
            DetailsRow::Section(text) | DetailsRow::Value(text) => text.clone(),
            DetailsRow::Toggle { label, enabled } => {
                format!("{label} {}", if *enabled { "ON" } else { "OFF" })
            }
        }));
        lines
    }
}

/// The host-testable view model for the native status and details screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiViewModel {
    pub status_bar: StatusBarViewModel,
    pub details: DetailsViewModel,
}

impl UiViewModel {
    pub fn new(status_bar: StatusBarViewModel, details: DetailsViewModel) -> Self {
        Self {
            status_bar,
            details,
        }
    }
}

pub fn draw_screen_with_feedback(
    display: &mut NativeDisplay,
    lines: &[String],
    feedback: Option<&str>,
    refresh_region: DisplayRegion,
    waveform: WaveformMode,
    wait_for_completion: bool,
    force_refresh: bool,
) -> std::io::Result<()> {
    let frame = if lines
        .get(1)
        .is_some_and(|line| line == "Details / Settings")
    {
        let status = lines
            .first()
            .map(|line| StatusBarViewModel::from_wire_line(line))
            .unwrap_or_default();
        let details = DetailsViewModel::from_lines(&lines[1..]);
        render_details_settings(
            &status,
            &details,
            display.width() as usize,
            display.height() as usize,
            None,
        )
    } else {
        render_screen_with_feedback(
            lines,
            feedback,
            display.width() as usize,
            display.height() as usize,
        )
    };
    display.draw_frame_with_waveform(
        &frame,
        refresh_region,
        waveform,
        wait_for_completion,
        force_refresh,
    )
}

/// Draw the status and Details / Settings view from its extracted model.
pub fn draw_screen_view(
    display: &mut NativeDisplay,
    view: &UiViewModel,
    refresh_region: DisplayRegion,
    waveform: WaveformMode,
    wait_for_completion: bool,
    force_refresh: bool,
) -> std::io::Result<()> {
    draw_screen_view_with_pressed_action(
        display,
        view,
        refresh_region,
        waveform,
        wait_for_completion,
        force_refresh,
        None,
    )
}

pub fn draw_screen_view_with_pressed_action(
    display: &mut NativeDisplay,
    view: &UiViewModel,
    refresh_region: DisplayRegion,
    waveform: WaveformMode,
    wait_for_completion: bool,
    force_refresh: bool,
    pressed_action: Option<DetailsAction>,
) -> std::io::Result<()> {
    let frame = render_details_settings(
        &view.status_bar,
        &view.details,
        display.width() as usize,
        display.height() as usize,
        pressed_action,
    );
    display.draw_frame_with_waveform(
        &frame,
        refresh_region,
        waveform,
        wait_for_completion,
        force_refresh,
    )
}

/// Render a complete host frame containing only the status bar.
#[cfg(test)]
pub fn render_status_bar_host(status: &StatusBarViewModel) -> Vec<u8> {
    let mut frame = white_frame(SCREEN_WIDTH, SCREEN_HEIGHT);
    let mut canvas = DisplayCanvas::new(
        &mut frame,
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
        SCREEN_WIDTH.saturating_mul(2),
        0,
        0,
    );
    draw_status_bar_model(&mut canvas, status);
    frame
}

/// Render the complete 600x800 Details / Settings host frame.
#[cfg(test)]
pub fn render_details_settings_host(view: &UiViewModel) -> Vec<u8> {
    render_details_settings(
        &view.status_bar,
        &view.details,
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
        None,
    )
}

/// Render a host Details / Settings frame with one action visibly pressed.
#[cfg(test)]
pub fn render_details_settings_host_pressed(
    view: &UiViewModel,
    pressed_action: DetailsAction,
) -> Vec<u8> {
    render_details_settings(
        &view.status_bar,
        &view.details,
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
        Some(pressed_action),
    )
}

pub(crate) fn render_details_settings_frame(
    view: &UiViewModel,
    width: usize,
    height: usize,
) -> Vec<u8> {
    render_details_settings(&view.status_bar, &view.details, width, height, None)
}

fn render_details_settings(
    status: &StatusBarViewModel,
    details: &DetailsViewModel,
    width: usize,
    height: usize,
    pressed_action: Option<DetailsAction>,
) -> Vec<u8> {
    let mut frame = white_frame(width, height);
    let mut canvas = DisplayCanvas::new(&mut frame, width, height, width.saturating_mul(2), 0, 0);
    draw_status_bar_model(&mut canvas, status);
    draw_details_model(&mut canvas, details, pressed_action);
    frame
}

fn white_frame(width: usize, height: usize) -> Vec<u8> {
    let mut frame = vec![0u8; width.saturating_mul(height).saturating_mul(2)];
    for pixel in frame.chunks_exact_mut(2) {
        pixel.copy_from_slice(&WHITE.to_ne_bytes());
    }
    frame
}

/// Convert a tightly packed native RGB565 frame to a deterministic grayscale
/// PNG suitable for host golden tests.
#[cfg(test)]
pub fn rgb565_to_png(frame: &[u8], width: usize, height: usize) -> io::Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "PNG dimensions must be non-zero",
        ));
    }
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(2))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "RGB565 frame is too large"))?;
    if frame.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "RGB565 frame has {} bytes; expected {expected}",
                frame.len()
            ),
        ));
    }

    let mut grayscale = Vec::with_capacity(width.saturating_mul(height));
    for pixel in frame.chunks_exact(2) {
        let value = u16::from_ne_bytes([pixel[0], pixel[1]]);
        let red = u32::from((value >> 11) & 0x1f) * 255 / 31;
        let green = u32::from((value >> 5) & 0x3f) * 255 / 63;
        let blue = u32::from(value & 0x1f) * 255 / 31;
        grayscale.push(((red * 299 + green * 587 + blue * 114 + 500) / 1000) as u8);
    }

    let mut output = Vec::new();
    output.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let width_u32 = u32::try_from(width)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "PNG width is too large"))?;
    let height_u32 = u32::try_from(height)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "PNG height is too large"))?;
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width_u32.to_be_bytes());
    header.extend_from_slice(&height_u32.to_be_bytes());
    header.extend_from_slice(&[8, 0, 0, 0, 0]);
    write_png_chunk(&mut output, b"IHDR", &header)?;

    let scanline_width = width
        .checked_add(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "PNG row is too large"))?;
    let mut scanlines = Vec::with_capacity(scanline_width.saturating_mul(height));
    for row in grayscale.chunks_exact(width) {
        scanlines.push(0);
        scanlines.extend_from_slice(row);
    }
    let mut compressed = Vec::new();
    write_zlib_stored(&scanlines, &mut compressed);
    write_png_chunk(&mut output, b"IDAT", &compressed)?;
    write_png_chunk(&mut output, b"IEND", &[])?;
    Ok(output)
}

#[cfg(test)]
fn write_png_chunk(output: &mut impl Write, kind: &[u8; 4], data: &[u8]) -> io::Result<()> {
    let length = u32::try_from(data.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "PNG chunk is too large"))?;
    output.write_all(&length.to_be_bytes())?;
    output.write_all(kind)?;
    output.write_all(data)?;
    output.write_all(&png_crc(kind, data).to_be_bytes())
}

#[cfg(test)]
fn write_zlib_stored(data: &[u8], output: &mut Vec<u8>) {
    output.extend_from_slice(&[0x78, 0x01]);
    if data.is_empty() {
        output.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    } else {
        let mut offset = 0;
        while offset < data.len() {
            let end = offset.saturating_add(u16::MAX as usize).min(data.len());
            output.push(u8::from(end == data.len()));
            let length = (end - offset) as u16;
            output.extend_from_slice(&length.to_le_bytes());
            output.extend_from_slice(&(!length).to_le_bytes());
            output.extend_from_slice(&data[offset..end]);
            offset = end;
        }
    }
    output.extend_from_slice(&adler32(data).to_be_bytes());
}

#[cfg(test)]
fn adler32(data: &[u8]) -> u32 {
    const MODULO: u32 = 65_521;
    let (mut low, mut high) = (1u32, 0u32);
    for byte in data {
        low = (low + u32::from(*byte)) % MODULO;
        high = (high + low) % MODULO;
    }
    high << 16 | low
}

#[cfg(test)]
fn png_crc(kind: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// Render the short-lived reader authorization screen.
///
/// Only the public approval URL reaches this renderer. The polling secret and
/// the eventual reader bearer token stay inside the synchronization client.
pub fn draw_authorization_qr(
    display: &mut NativeDisplay,
    approval_url: &str,
    status_line: &str,
) -> std::io::Result<()> {
    let frame = render_authorization_qr(
        display.width() as usize,
        display.height() as usize,
        approval_url,
        status_line,
    )?;
    display.draw_frame_with_waveform(
        &frame,
        DisplayRegion::full(display.width(), display.height()),
        WaveformMode::Gc16,
        true,
        true,
    )
}

pub(crate) fn render_authorization_qr(
    width: usize,
    height: usize,
    approval_url: &str,
    status_line: &str,
) -> std::io::Result<Vec<u8>> {
    let qr = QrMatrix::encode(approval_url).map_err(|error| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string())
    })?;
    let mut frame = vec![0u8; width.saturating_mul(height).saturating_mul(2)];
    let mut canvas = DisplayCanvas::new(&mut frame, width, height, width.saturating_mul(2), 0, 0);
    canvas.fill(WHITE);
    draw_status_bar(&mut canvas, &[status_line.to_owned()]);
    draw_text_centered_font(
        &mut canvas,
        58,
        "Scan to authorize",
        &FONT_10X20,
        Rgb565::BLACK,
    );

    let quiet_zone = 4usize;
    let footer_height = 58usize;
    let available_width = width.saturating_sub(32);
    let available_height = height.saturating_sub(92 + footer_height);
    let module_count = qr.size().saturating_add(quiet_zone * 2);
    let scale = available_width
        .min(available_height)
        .checked_div(module_count)
        .unwrap_or(0);
    if scale == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "display is too small for the authorization QR code",
        ));
    }
    let qr_size = module_count.saturating_mul(scale);
    let left = width.saturating_sub(qr_size) / 2;
    let top = 82usize;
    for y in 0..qr.size() {
        for x in 0..qr.size() {
            if !qr.is_dark(x, y) {
                continue;
            }
            canvas.fill_rect(
                left.saturating_add((x + quiet_zone) * scale),
                top.saturating_add((y + quiet_zone) * scale),
                scale,
                scale,
                BLACK,
            );
        }
    }
    draw_text_centered(
        &mut canvas,
        height.saturating_sub(48),
        "Approve on your trusted device",
        Rgb565::BLACK,
    );
    Ok(frame)
}

/// Render and present a bounded grayscale calibration pattern.
///
/// This command intentionally leaves the pattern in the framebuffer after the
/// wait. It is a physical panel test, not a reversible framebuffer probe.
pub fn display_test_to(
    path: &Path,
    wait_after_update: Duration,
    waveform: WaveformMode,
) -> std::io::Result<()> {
    let mut display = NativeDisplay::open(path)?;
    draw_display_test(&mut display, waveform, true, true)?;
    eprintln!(
        "display-test: pattern={}x{} waveform={} wait_seconds={}",
        display.width(),
        display.height(),
        waveform.label(),
        wait_after_update.as_secs(),
    );
    thread::sleep(wait_after_update);
    eprintln!("display-test: pattern left visible; no framebuffer restore performed");
    Ok(())
}

/// Draw the full-screen calibration pattern into an already-open display.
pub fn draw_display_test(
    display: &mut NativeDisplay,
    waveform: WaveformMode,
    wait_for_completion: bool,
    force_refresh: bool,
) -> std::io::Result<()> {
    draw_display_test_frame(display, waveform, wait_for_completion, force_refresh, false)
}

/// Draw the interactive calibration pattern used by the native shell.
pub(crate) fn draw_interactive_display_test(
    display: &mut NativeDisplay,
    waveform: WaveformMode,
    wait_for_completion: bool,
    force_refresh: bool,
) -> std::io::Result<()> {
    draw_display_test_frame(display, waveform, wait_for_completion, force_refresh, true)
}

fn draw_display_test_frame(
    display: &mut NativeDisplay,
    waveform: WaveformMode,
    wait_for_completion: bool,
    force_refresh: bool,
    show_return_hint: bool,
) -> std::io::Result<()> {
    let frame = if show_return_hint {
        render_interactive_display_test(display.width() as usize, display.height() as usize)
    } else {
        render_display_test(display.width() as usize, display.height() as usize)
    };
    display.draw_frame_with_waveform(
        &frame,
        DisplayRegion::full(display.width(), display.height()),
        waveform,
        wait_for_completion,
        force_refresh,
    )
}

/// Build the packed RGB565 frame used by `display-test`.
pub fn render_display_test(width: usize, height: usize) -> Vec<u8> {
    render_display_test_frame(width, height, false)
}

pub(crate) fn render_interactive_display_test(width: usize, height: usize) -> Vec<u8> {
    render_display_test_frame(width, height, true)
}

fn render_display_test_frame(width: usize, height: usize, show_return_hint: bool) -> Vec<u8> {
    let frame_len = width.saturating_mul(height).saturating_mul(2);
    let mut frame = vec![0u8; frame_len];
    let mut canvas = DisplayCanvas::new(&mut frame, width, height, width.saturating_mul(2), 0, 0);
    draw_display_test_contents(&mut canvas, show_return_hint);
    frame
}

fn draw_display_test_contents(canvas: &mut DisplayCanvas<'_>, show_return_hint: bool) {
    let width = canvas.width();
    let margin = width.min(16);

    canvas.fill(WHITE);
    canvas.fill_rect(0, 0, width, DISPLAY_TEST_HEADER_HEIGHT, BLACK);
    draw_text_font(
        canvas,
        margin,
        15,
        "PRS-T1 DISPLAY TEST",
        &FONT_10X20,
        Rgb565::WHITE,
    );
    draw_text_font(
        canvas,
        width.saturating_sub(112),
        19,
        "RGB565 / GRAY",
        &FONT_8X13,
        Rgb565::WHITE,
    );

    draw_text_font(
        canvas,
        margin,
        58,
        "FILL / TEXT CONTRAST",
        &FONT_8X13_BOLD,
        Rgb565::BLACK,
    );
    draw_contrast_swatches(canvas, margin);

    draw_text_font(
        canvas,
        margin,
        DISPLAY_TEST_GRADIENT_TOP.saturating_sub(20),
        "HORIZONTAL GRADIENT  0 -> 255",
        &FONT_8X13_BOLD,
        Rgb565::BLACK,
    );
    draw_horizontal_gradient(
        canvas,
        margin,
        DISPLAY_TEST_GRADIENT_TOP,
        width.saturating_sub(margin.saturating_mul(2)),
        DISPLAY_TEST_GRADIENT_HEIGHT,
    );

    let panel_gap = 8.min(width);
    let panel_width = width
        .saturating_sub(margin.saturating_mul(2))
        .saturating_sub(panel_gap)
        / 2;
    let left_panel = margin;
    let right_panel = left_panel
        .saturating_add(panel_width)
        .saturating_add(panel_gap);
    draw_text_font(
        canvas,
        left_panel,
        DISPLAY_TEST_PANELS_TOP.saturating_sub(20),
        "VERTICAL GRADIENT",
        &FONT_8X13_BOLD,
        Rgb565::BLACK,
    );
    draw_text_font(
        canvas,
        right_panel,
        DISPLAY_TEST_PANELS_TOP.saturating_sub(20),
        "LINE THICKNESS",
        &FONT_8X13_BOLD,
        Rgb565::BLACK,
    );
    draw_vertical_gradient(
        canvas,
        left_panel,
        DISPLAY_TEST_PANELS_TOP,
        panel_width,
        DISPLAY_TEST_PANELS_HEIGHT,
    );
    draw_line_samples(
        canvas,
        right_panel,
        DISPLAY_TEST_PANELS_TOP,
        panel_width,
        DISPLAY_TEST_PANELS_HEIGHT,
    );

    draw_text_font(
        canvas,
        margin,
        DISPLAY_TEST_RAMP_TOP.saturating_sub(20),
        "16-LEVEL GRAYSCALE RAMP",
        &FONT_8X13_BOLD,
        Rgb565::BLACK,
    );
    draw_grayscale_ramp(
        canvas,
        margin,
        DISPLAY_TEST_RAMP_TOP,
        width.saturating_sub(margin.saturating_mul(2)),
        DISPLAY_TEST_RAMP_HEIGHT,
    );

    draw_text_font(
        canvas,
        margin,
        DISPLAY_TEST_RAMP_TOP
            .saturating_add(DISPLAY_TEST_RAMP_HEIGHT)
            .saturating_add(14),
        "Compare framebuffer capture with the physical panel.",
        &FONT_8X13,
        Rgb565::BLACK,
    );
    if show_return_hint {
        draw_text_font(
            canvas,
            margin,
            DISPLAY_TEST_RAMP_TOP
                .saturating_add(DISPLAY_TEST_RAMP_HEIGHT)
                .saturating_add(30),
            "Short MENU press returns to Details / Settings.",
            &FONT_8X13,
            Rgb565::BLACK,
        );
    }
}

fn draw_contrast_swatches(canvas: &mut DisplayCanvas<'_>, margin: usize) {
    let width = canvas.width();
    let available = width.saturating_sub(margin.saturating_mul(2));
    let gap = DISPLAY_TEST_GRID_GAP.min(available);
    let tile_width = available.saturating_sub(gap.saturating_mul(2)) / 3;
    let values = [0u8, 32, 64, 96, 128, 160, 192, 224, 255];

    for (index, gray) in values.into_iter().enumerate() {
        let column = index % 3;
        let row = index / 3;
        let left = margin.saturating_add(column.saturating_mul(tile_width.saturating_add(gap)));
        let top = DISPLAY_TEST_GRID_TOP
            .saturating_add(row.saturating_mul(DISPLAY_TEST_GRID_TILE_HEIGHT.saturating_add(gap)));
        let fill = gray565(gray);
        canvas.fill_rect(left, top, tile_width, DISPLAY_TEST_GRID_TILE_HEIGHT, fill);
        canvas.stroke_rect(
            left,
            top,
            tile_width,
            DISPLAY_TEST_GRID_TILE_HEIGHT,
            if gray < 128 { WHITE } else { BLACK },
        );
        draw_text_font(
            canvas,
            left.saturating_add(8),
            top.saturating_add(8),
            "BLACK TEXT",
            &FONT_8X13,
            Rgb565::BLACK,
        );
        draw_text_font(
            canvas,
            left.saturating_add(8),
            top.saturating_add(29),
            "WHITE TEXT",
            &FONT_8X13,
            Rgb565::WHITE,
        );
        let label = format!("FILL {gray:03}");
        draw_text_font(
            canvas,
            left.saturating_add(8),
            top.saturating_add(50),
            &label,
            &FONT_8X13_BOLD,
            if gray < 128 {
                Rgb565::WHITE
            } else {
                Rgb565::BLACK
            },
        );
    }
}

fn draw_horizontal_gradient(
    canvas: &mut DisplayCanvas<'_>,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
) {
    for column in 0..width {
        let gray = gradient_value(column, width);
        canvas.fill_rect(left.saturating_add(column), top, 1, height, gray565(gray));
    }
}

fn draw_vertical_gradient(
    canvas: &mut DisplayCanvas<'_>,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let gray = gradient_value(row, height);
        canvas.fill_rect(left, top.saturating_add(row), width, 1, gray565(gray));
    }
}

fn draw_line_samples(
    canvas: &mut DisplayCanvas<'_>,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
) {
    canvas.fill_rect(left, top, width, height, WHITE);
    canvas.stroke_rect(left, top, width, height, BLACK);
    let line_left = left.saturating_add(64.min(width));
    let line_width = width.saturating_sub(76.min(width));
    for (index, thickness) in [1usize, 2, 4, 8].into_iter().enumerate() {
        let line_top = top.saturating_add(12 + index.saturating_mul(27));
        draw_text_font(
            canvas,
            left.saturating_add(8),
            line_top.saturating_sub(3),
            &format!("{thickness} px"),
            &FONT_8X13,
            Rgb565::BLACK,
        );
        canvas.fill_rect(line_left, line_top, line_width, thickness, BLACK);
    }
}

fn draw_grayscale_ramp(
    canvas: &mut DisplayCanvas<'_>,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
) {
    for step in 0..16 {
        let start = width.saturating_mul(step) / 16;
        let end = width.saturating_mul(step + 1) / 16;
        canvas.fill_rect(
            left.saturating_add(start),
            top,
            end.saturating_sub(start),
            height,
            gray565((step * 255 / 15) as u8),
        );
    }
    canvas.stroke_rect(left, top, width, height, BLACK);
}

fn gradient_value(position: usize, length: usize) -> u8 {
    if length <= 1 {
        0
    } else {
        (position.saturating_mul(255) / (length - 1)) as u8
    }
}

fn gray565(gray: u8) -> u16 {
    let gray = u16::from(gray);
    let red_blue = (gray.saturating_mul(31) + 127) / 255;
    let green = (gray.saturating_mul(63) + 127) / 255;
    (red_blue << 11) | (green << 5) | red_blue
}

fn draw_screen_contents_with_feedback(
    canvas: &mut DisplayCanvas<'_>,
    lines: &[String],
    feedback: Option<&str>,
) {
    canvas.fill(WHITE);
    draw_status_bar(canvas, lines);
    if let Some(feedback) = feedback {
        draw_reader_feedback(canvas, feedback);
    }

    let is_details = lines
        .get(1)
        .is_some_and(|line| line == "Details / Settings");
    if is_details {
        let details = DetailsViewModel::from_lines(&lines[1..]);
        draw_details_model(canvas, &details, None);
        return;
    }

    let line_step = CONTENT_LINE_STEP;
    for (index, line) in lines.iter().skip(1).enumerate() {
        let y = CONTENT_TOP.saturating_add(index.saturating_mul(line_step));
        if index == 0 {
            draw_text_centered_font(canvas, y, line, &FONT_10X20, Rgb565::BLACK);
        } else {
            draw_text_centered(canvas, y, line, Rgb565::BLACK);
        }
    }
}

fn draw_details_model(
    canvas: &mut DisplayCanvas<'_>,
    details: &DetailsViewModel,
    pressed_action: Option<DetailsAction>,
) {
    draw_text_font(
        canvas,
        24,
        CONTENT_TOP,
        &details.title,
        &FONT_10X20,
        Rgb565::BLACK,
    );
    for (index, row) in details.rows.iter().enumerate() {
        let y = CONTENT_TOP.saturating_add((index + 1).saturating_mul(DETAILS_LINE_STEP));
        if y.saturating_add(16) > DETAILS_ACTION_HEADER_TOP.saturating_sub(4) {
            break;
        }
        match row {
            DetailsRow::Section(text) => draw_section_heading(canvas, y, text),
            DetailsRow::Value(text) => draw_text(canvas, 24, y, text),
            DetailsRow::Toggle { label, enabled } => draw_debug_toggle(canvas, y, label, *enabled),
        }
    }
    draw_text_font(
        canvas,
        24,
        DETAILS_ACTION_HEADER_TOP,
        "Actions",
        &FONT_8X13_BOLD,
        Rgb565::BLACK,
    );
    canvas.fill_rect(
        24,
        DETAILS_ACTION_HEADER_TOP.saturating_add(16),
        canvas.width().saturating_sub(48),
        1,
        BLACK,
    );
    draw_details_actions(canvas, pressed_action);
}

/// Render a logical screen into the format expected by the EPDC standby
/// framebuffer ioctl. This buffer has no virtual-screen padding or offsets.
#[cfg(test)]
pub fn standby_screen(lines: &[String], width: usize, height: usize) -> Vec<u8> {
    render_screen(lines, width, height)
}

#[cfg(test)]
fn render_screen(lines: &[String], width: usize, height: usize) -> Vec<u8> {
    render_screen_with_feedback(lines, None, width, height)
}

fn render_screen_with_feedback(
    lines: &[String],
    feedback: Option<&str>,
    width: usize,
    height: usize,
) -> Vec<u8> {
    let mut image = vec![0u8; width.saturating_mul(height).saturating_mul(2)];
    {
        let mut canvas =
            DisplayCanvas::new(&mut image, width, height, width.saturating_mul(2), 0, 0);
        draw_screen_contents_with_feedback(&mut canvas, lines, feedback);
    }
    image
}

pub(crate) fn draw_status_bar(canvas: &mut DisplayCanvas<'_>, lines: &[String]) {
    let Some(line) = lines.first() else {
        canvas.fill_rect(0, 0, canvas.width(), STATUS_BAR_HEIGHT, BLACK);
        return;
    };
    draw_status_bar_model(canvas, &StatusBarViewModel::from_wire_line(line));
}

fn draw_status_bar_model(canvas: &mut DisplayCanvas<'_>, view: &StatusBarViewModel) {
    let width = canvas.width();
    canvas.fill_rect(0, 0, width, STATUS_BAR_HEIGHT, BLACK);

    let side_margin = STATUS_BAR_SIDE_MARGIN.min(width / 2);
    let clock_width = STATUS_BAR_CLOCK_WIDTH.min(width.saturating_sub(side_margin * 2));
    let clock_left = width.saturating_sub(side_margin + clock_width);
    let mut left = side_margin;

    if !view.battery.is_empty() {
        left = left.saturating_add(draw_status_value(canvas, left, &view.battery, BATTERY_ICON));
        left = left.saturating_add(8);
    }
    for (value, sprite) in [
        (&view.wifi, WIFI_ICON),
        (&view.usb, USB_ICON),
        (&view.adb, ADB_ICON),
    ] {
        if status_icon_is_on(value) {
            draw_icon_sprite(canvas, left, 14, sprite);
            left = left.saturating_add(20 + STATUS_ICON_GAP);
        }
    }

    if !view.mode.is_empty() {
        let mode_left = clock_left.saturating_sub(STATUS_MODE_WIDTH + 16);
        draw_text_centered_in_rect_font(
            canvas,
            mode_left,
            0,
            STATUS_MODE_WIDTH,
            STATUS_BAR_HEIGHT,
            &view.mode,
            &FONT_8X13_BOLD,
            Rgb565::WHITE,
        );
    }
    if !view.clock.is_empty() {
        draw_status_value(canvas, clock_left, &view.clock, CLOCK_ICON);
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
        "Settings"
            | "Power"
            | "Connectivity"
            | "Synchronization"
            | "System"
            | "Storage"
            | "Input"
            | "Diagnostics"
    )
}

fn debug_toggle_state(line: &str) -> Option<bool> {
    line.strip_prefix("Debug messages ")
        .and_then(|value| match value {
            "ON" => Some(true),
            "OFF" => Some(false),
            _ => None,
        })
}

fn draw_debug_toggle(canvas: &mut DisplayCanvas<'_>, y: usize, label: &str, enabled: bool) {
    draw_text(canvas, 24, y, label);
    let left = 430;
    let top = y.saturating_sub(3);
    let width = canvas.width().saturating_sub(left + 24);
    canvas.stroke_rect(left, top, width, 22, BLACK);
    draw_text_centered_in_rect(
        canvas,
        left,
        top,
        width,
        22,
        if enabled { "ON" } else { "OFF" },
        Rgb565::BLACK,
    );
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

fn draw_details_actions(canvas: &mut DisplayCanvas<'_>, pressed_action: Option<DetailsAction>) {
    let width = canvas.width();
    let height = canvas.height();
    for action in DetailsAction::ALL {
        let region = details_action_region(action, width, height);
        let margin = region.left as usize;
        let top = region.top as usize;
        let button_width = region.width as usize;
        let button_height = region.height as usize;
        if button_width == 0 || button_height == 0 {
            continue;
        }
        let pressed = pressed_action == Some(action);
        if pressed {
            canvas.fill_rect(margin, top, button_width, button_height, BLACK);
        } else {
            canvas.stroke_rect(margin, top, button_width, button_height, BLACK);
            if action.is_destructive() && button_width > 6 && button_height > 6 {
                canvas.stroke_rect(
                    margin.saturating_add(3),
                    top.saturating_add(3),
                    button_width.saturating_sub(6),
                    button_height.saturating_sub(6),
                    BLACK,
                );
            }
        }
        draw_text_centered_in_rect(
            canvas,
            margin,
            top,
            button_width,
            button_height,
            action.label(),
            if pressed {
                Rgb565::WHITE
            } else {
                Rgb565::BLACK
            },
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
    use super::{
        details_action_at, details_action_region, gray565, render_authorization_qr,
        render_details_settings_host, render_details_settings_host_pressed, render_display_test,
        render_status_bar_host, rgb565_to_png, standby_screen, DetailsAction, DetailsRow,
        DetailsViewModel, StatusBarViewModel, UiViewModel, BLACK, DETAILS_ACTION_HEADER_TOP,
        DETAILS_ACTION_HEIGHT, DETAILS_ACTION_TOP, DETAILS_LINE_STEP, SCREEN_HEIGHT, SCREEN_WIDTH,
        WHITE,
    };
    use std::env;
    use std::fs;
    use std::path::Path;

    fn pixel(frame: &[u8], width: usize, x: usize, y: usize) -> u16 {
        let offset = (y * width + x) * 2;
        u16::from_ne_bytes([frame[offset], frame[offset + 1]])
    }

    fn screenshot_view(debug_messages: bool) -> UiViewModel {
        let status_bar = StatusBarViewModel {
            battery: "87%".into(),
            wifi: "UP".into(),
            usb: "ON".into(),
            adb: "ON".into(),
            mode: "SYNCING".into(),
            clock: "12:34".into(),
        };
        let rows = vec![
            DetailsRow::Section("Settings".into()),
            DetailsRow::Toggle {
                label: "Debug messages".into(),
                enabled: debug_messages,
            },
            DetailsRow::Section("Synchronization".into()),
            DetailsRow::Value("Sync active  Failure none".into()),
            DetailsRow::Section("Power".into()),
            DetailsRow::Value("Battery 87% Charging  Temp 24 C".into()),
            DetailsRow::Value("Health Good  Voltage 4.20 V  AC On USB On".into()),
            DetailsRow::Section("Connectivity".into()),
            DetailsRow::Value("WiFi wlan0 Up  Supplicant Completed".into()),
            DetailsRow::Value("USB On  Gadget Configured  ADB On  Functions Adb".into()),
            DetailsRow::Section("Storage".into()),
            DetailsRow::Value("Data 123456 KiB".into()),
            DetailsRow::Value("SD card 654321 KiB".into()),
            DetailsRow::Section("Diagnostics".into()),
            DetailsRow::Value("System: FB ACTIVE  Rotate 0  zygote STOP  dispd STOP".into()),
            DetailsRow::Value("Runtime: Wake yes  Date 21 Sep 2026 12:34".into()),
            DetailsRow::Value("Input: 0 touch  0 key  Power none".into()),
        ];
        UiViewModel::new(
            status_bar,
            DetailsViewModel::new("Details / Settings", rows),
        )
    }

    fn assert_png_golden(name: &str, actual: &[u8]) {
        let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/goldens")
            .join(format!("{name}.png"));
        if env::var_os("PRS_T1_UPDATE_GOLDENS").is_some() {
            fs::create_dir_all(golden.parent().expect("golden has a parent"))
                .expect("create screenshot golden directory");
            fs::write(&golden, actual).expect("write screenshot golden");
            return;
        }

        let expected = fs::read(&golden).unwrap_or_else(|error| {
            let failure = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target")
                .join("prs-t1-agent-golden-failures")
                .join(format!("{name}.png"));
            fs::create_dir_all(failure.parent().expect("failure has a parent"))
                .expect("create screenshot failure directory");
            fs::write(&failure, actual).expect("write screenshot failure");
            panic!(
                "missing native UI golden {} ({error}); rendered output was written to {}",
                golden.display(),
                failure.display()
            );
        });
        if expected != actual {
            let failure = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target")
                .join("prs-t1-agent-golden-failures")
                .join(format!("{name}.png"));
            fs::create_dir_all(failure.parent().expect("failure has a parent"))
                .expect("create screenshot failure directory");
            fs::write(&failure, actual).expect("write screenshot failure");
            panic!(
                "native UI golden mismatch for {name}; rendered output was written to {}",
                failure.display()
            );
        }
    }

    #[test]
    fn status_bar_host_renderer_matches_png_golden() {
        let frame = render_status_bar_host(&screenshot_view(false).status_bar);
        assert_eq!(frame.len(), SCREEN_WIDTH * SCREEN_HEIGHT * 2);
        let png = rgb565_to_png(&frame, SCREEN_WIDTH, SCREEN_HEIGHT).expect("encode status PNG");
        assert_png_golden("status-bar", &png);
    }

    #[test]
    fn details_settings_host_renderer_matches_png_golden() {
        let frame = render_details_settings_host(&screenshot_view(false));
        assert_eq!(frame.len(), SCREEN_WIDTH * SCREEN_HEIGHT * 2);
        let png = rgb565_to_png(&frame, SCREEN_WIDTH, SCREEN_HEIGHT).expect("encode details PNG");
        assert_png_golden("details-settings", &png);
    }

    #[test]
    fn status_bar_syncing_host_renderer_matches_png_golden() {
        let frame = render_status_bar_host(&screenshot_view(false).status_bar);
        let png = rgb565_to_png(&frame, SCREEN_WIDTH, SCREEN_HEIGHT).expect("encode syncing PNG");
        assert_png_golden("status-bar-syncing", &png);
    }

    #[test]
    fn details_settings_debug_toggle_host_renderer_matches_png_golden() {
        let frame = render_details_settings_host(&screenshot_view(true));
        let png =
            rgb565_to_png(&frame, SCREEN_WIDTH, SCREEN_HEIGHT).expect("encode debug settings PNG");
        assert_png_golden("details-settings-debug", &png);
    }

    #[test]
    fn pressed_action_renderer_changes_only_the_action_visual_state() {
        let view = screenshot_view(false);
        let normal = render_details_settings_host(&view);
        let pressed = render_details_settings_host_pressed(&view, DetailsAction::SyncNow);

        assert_ne!(normal, pressed);
        let pressed_png = rgb565_to_png(&pressed, SCREEN_WIDTH, SCREEN_HEIGHT)
            .expect("encode pressed details PNG");
        assert_png_golden("details-settings-pressed", &pressed_png);
        assert_eq!(
            pixel(&normal, SCREEN_WIDTH, 30, DETAILS_ACTION_TOP + 10),
            WHITE
        );
        assert_eq!(
            pixel(&pressed, SCREEN_WIDTH, 30, DETAILS_ACTION_TOP + 10),
            BLACK
        );
        assert_eq!(
            pixel(
                &normal,
                SCREEN_WIDTH,
                30,
                DETAILS_ACTION_TOP + DETAILS_ACTION_HEIGHT + 2
            ),
            WHITE
        );
        assert_eq!(
            pixel(
                &pressed,
                SCREEN_WIDTH,
                30,
                DETAILS_ACTION_TOP + DETAILS_ACTION_HEIGHT + 2
            ),
            WHITE
        );
    }

    #[test]
    fn details_content_fits_before_the_dedicated_action_pane() {
        let view = screenshot_view(false);
        for (index, _) in view.details.rows.iter().enumerate() {
            let top = super::CONTENT_TOP + (index + 1) * DETAILS_LINE_STEP;
            assert!(
                top + 16 <= DETAILS_ACTION_HEADER_TOP - 4,
                "row {index} at y={top} intersects the action pane"
            );
        }
        let actions = DetailsAction::ALL.map(|action| details_action_region(action, 600, 800));
        for pair in actions.windows(2) {
            assert!(pair[0].bottom() <= pair[1].top);
        }
    }

    #[test]
    fn action_hit_testing_matches_rendered_rectangles() {
        for action in DetailsAction::ALL {
            let region = details_action_region(action, SCREEN_WIDTH, SCREEN_HEIGHT);
            assert_eq!(
                details_action_at(
                    region.left as i32 + 1,
                    region.top as i32 + 1,
                    SCREEN_WIDTH,
                    SCREEN_HEIGHT
                ),
                Some(action)
            );
            assert_eq!(
                details_action_at(
                    region.right() as i32,
                    region.top as i32 + 1,
                    SCREEN_WIDTH,
                    SCREEN_HEIGHT
                ),
                None
            );
        }
    }

    #[test]
    fn details_view_model_preserves_the_existing_device_frame() {
        let view = screenshot_view(false);
        let mut lines = vec![view.status_bar.to_wire_line()];
        lines.extend(view.details.to_lines());

        let expected = standby_screen(&lines, SCREEN_WIDTH, SCREEN_HEIGHT);
        let actual = render_details_settings_host(&view);
        let differences = expected
            .chunks_exact(2)
            .zip(actual.chunks_exact(2))
            .enumerate()
            .filter(|(_, (expected, actual))| expected != actual)
            .map(|(index, _)| (index % SCREEN_WIDTH, index / SCREEN_WIDTH))
            .collect::<Vec<_>>();
        assert!(
            differences.is_empty(),
            "{} pixels differ; first differences: {:?}",
            differences.len(),
            &differences[..differences.len().min(12)]
        );
    }

    #[test]
    fn details_content_stays_above_the_action_stack() {
        let view = screenshot_view(false);
        let last_baseline = super::CONTENT_TOP + view.details.rows.len() * super::DETAILS_LINE_STEP;
        let text_bottom = last_baseline + 16;
        assert!(
            text_bottom < super::DETAILS_SYNC_TOP,
            "details text reaches y={text_bottom}, before action stack at y={}",
            super::DETAILS_SYNC_TOP
        );
        assert!(
            super::DETAILS_DEBUG_TOP + super::DETAILS_DEBUG_HEIGHT < super::DETAILS_SYNC_TOP,
            "debug toggle overlaps action stack"
        );
    }

    #[test]
    fn rgb565_png_conversion_rejects_wrong_frame_size() {
        let error = rgb565_to_png(&[0, 0], 2, 2).expect_err("short RGB565 frame");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

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

    #[test]
    fn authorization_qr_is_a_full_frame_with_dark_modules() {
        let frame = render_authorization_qr(
            600,
            800,
            "https://reader.example/a/request-1",
            "||||SYNCING|",
        )
        .unwrap();

        assert_eq!(frame.len(), 600 * 800 * 2);
        assert!(frame
            .chunks(2)
            .any(|pixel| pixel != 0xff_ffu16.to_ne_bytes()));
    }

    #[test]
    fn display_test_contains_contrast_swatches_and_gradients() {
        let frame = render_display_test(600, 800);

        assert_eq!(frame.len(), 600 * 800 * 2);
        assert_eq!(pixel(&frame, 600, 1, 1), gray565(0));
        assert_eq!(pixel(&frame, 600, 180, 100), gray565(0));
        assert_eq!(pixel(&frame, 600, 360, 100), gray565(32));
        assert_eq!(pixel(&frame, 600, 560, 100), gray565(64));
        assert_eq!(pixel(&frame, 600, 180, 190), gray565(96));
        assert_eq!(pixel(&frame, 600, 180, 278), gray565(192));
        assert_eq!(pixel(&frame, 600, 16, 395), gray565(0));
        assert_eq!(pixel(&frame, 600, 583, 395), gray565(255));
        assert_eq!(pixel(&frame, 600, 20, 464), gray565(0));
        assert_eq!(pixel(&frame, 600, 20, 583), gray565(255));
    }

    #[test]
    fn display_test_line_samples_have_requested_thicknesses() {
        let frame = render_display_test(600, 800);

        for y in 476..477 {
            assert_eq!(pixel(&frame, 600, 380, y), gray565(0));
        }
        assert_eq!(pixel(&frame, 600, 380, 477), gray565(255));
        for y in 503..505 {
            assert_eq!(pixel(&frame, 600, 380, y), gray565(0));
        }
        assert_eq!(pixel(&frame, 600, 380, 505), gray565(255));
        for y in 530..534 {
            assert_eq!(pixel(&frame, 600, 380, y), gray565(0));
        }
        assert_eq!(pixel(&frame, 600, 380, 534), gray565(255));
        for y in 557..565 {
            assert_eq!(pixel(&frame, 600, 380, y), gray565(0));
        }
        assert_eq!(pixel(&frame, 600, 380, 565), gray565(255));
    }

    #[test]
    fn display_test_handles_empty_frame() {
        assert!(render_display_test(0, 0).is_empty());
    }
}
