use crate::damage::{self, DamageOptions};
use crate::orientation::ReaderOrientation;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::raw::{c_int, c_ulong, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;
use std::thread;
use std::time::{Duration, Instant};

const FBIOGET_VSCREENINFO: c_ulong = 0x4600;
const FBIOPUT_VSCREENINFO: c_ulong = 0x4601;
const FBIOGET_FSCREENINFO: c_ulong = 0x4602;
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const MAP_FAILED: *mut c_void = -1isize as *mut c_void;
const MAX_MAPPED_BYTES: usize = 128 * 1024 * 1024;
const NATIVE_DISPLAY_WIDTH: u32 = 600;
const NATIVE_DISPLAY_HEIGHT: u32 = 800;
const LANDSCAPE_DISPLAY_WIDTH: u32 = NATIVE_DISPLAY_HEIGHT;
const LANDSCAPE_DISPLAY_HEIGHT: u32 = NATIVE_DISPLAY_WIDTH;

// These are the MXC EPDC ioctls used by the T1's installed
// /system/lib/hw/gralloc.imx5x.so. The ioctl payload size is part of the
// kernel ABI, so keep the update struct at exactly 0x44 bytes.
const MXCFB_SET_AUTO_UPDATE_MODE: c_ulong = 0x4004_462d;
const MXCFB_SEND_UPDATE: c_ulong = 0x4044_462e;
const MXCFB_WAIT_FOR_UPDATE_COMPLETE: c_ulong = 0x4004_462f;
const MXCFB_WRITE_SSCREEN: c_ulong = 0x4004_463b;
const AUTO_UPDATE_MODE_REGION: u32 = 0;
const WAVEFORM_MODE_GC16: u32 = 2;
const UPDATE_MODE_PARTIAL: u32 = 0;
const UPDATE_MODE_FULL: u32 = 1;
const TEMP_USE_AMBIENT: i32 = 0x1000;
const TEST_UPDATE_MARKER: u32 = 1;
const TEST_RECT_WIDTH: u32 = 200;
const TEST_RECT_HEIGHT: u32 = 120;

/// Waveforms exposed by the T1's i.MX EPDC ABI. These values match the
/// waveform table used by the installed Sony gralloc implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveformMode {
    Du,
    Gc16,
    Gc4,
    A2,
}

impl WaveformMode {
    pub fn parse(value: &str) -> io::Result<Self> {
        match value.to_ascii_uppercase().as_str() {
            "DU" => Ok(Self::Du),
            "GC16" => Ok(Self::Gc16),
            "GC4" => Ok(Self::Gc4),
            "A2" => Ok(Self::A2),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown waveform {value:?}; expected DU, GC16, GC4, or A2"),
            )),
        }
    }

    fn value(self) -> u32 {
        match self {
            Self::Du => 1,
            Self::Gc16 => WAVEFORM_MODE_GC16,
            Self::Gc4 => 3,
            Self::A2 => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Du => "DU",
            Self::Gc16 => "GC16",
            Self::Gc4 => "GC4",
            Self::A2 => "A2",
        }
    }
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: c_ulong,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        flags: c_int,
        fd: c_int,
        offset: isize,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> c_int;
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct MxcfbRect {
    top: u32,
    left: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct MxcfbAltBufferData {
    phys_addr: u32,
    width: u32,
    height: u32,
    alt_update_region: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct MxcfbUpdateData {
    update_region: MxcfbRect,
    waveform_mode: u32,
    update_mode: u32,
    update_marker: u32,
    temperature: i32,
    flags: u32,
    alt_buffer_data: MxcfbAltBufferData,
    reserved: [u32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelInfo {
    pub offset: u32,
    pub length: u32,
}

/// A logical, visible-screen rectangle to submit to the T1 EPDC.
///
/// The framebuffer remains full-screen and RGB565; this rectangle controls
/// which part of the panel the update waveform touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayRegion {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

impl DisplayRegion {
    pub const fn new(left: u32, top: u32, width: u32, height: u32) -> Self {
        Self {
            left,
            top,
            width,
            height,
        }
    }

    pub const fn full(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }

    pub fn bounded(self, width: u32, height: u32) -> Self {
        let left = self.left.min(width);
        let top = self.top.min(height);
        Self {
            left,
            top,
            width: self.width.min(width.saturating_sub(left)),
            height: self.height.min(height.saturating_sub(top)),
        }
    }

    pub fn right(self) -> u32 {
        self.left.saturating_add(self.width)
    }

    pub fn bottom(self) -> u32 {
        self.top.saturating_add(self.height)
    }

    pub fn contains(self, other: Self) -> bool {
        other.left >= self.left
            && other.top >= self.top
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    fn as_mxcfb(self) -> MxcfbRect {
        MxcfbRect {
            top: self.top,
            left: self.left,
            width: self.width,
            height: self.height,
        }
    }
}

impl fmt::Display for ChannelInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "offset={} length={}", self.offset, self.length)
    }
}

#[derive(Debug, Clone)]
pub struct FramebufferInfo {
    pub device: PathBuf,
    pub driver: String,
    pub width: u32,
    pub height: u32,
    pub virtual_width: u32,
    pub virtual_height: u32,
    pub xoffset: u32,
    pub yoffset: u32,
    pub bits_per_pixel: u32,
    pub grayscale: u32,
    pub stride: u32,
    pub memory_bytes: u32,
    pub rotate: u32,
    red: ChannelInfo,
    green: ChannelInfo,
    blue: ChannelInfo,
    transp: ChannelInfo,
}

impl FramebufferInfo {
    pub fn print(&self) {
        println!("framebuffer={}", self.device.display());
        println!("driver={}", self.driver);
        println!("width={} height={}", self.width, self.height);
        println!(
            "virtual_width={} virtual_height={}",
            self.virtual_width, self.virtual_height
        );
        println!("xoffset={} yoffset={}", self.xoffset, self.yoffset);
        println!("bits_per_pixel={}", self.bits_per_pixel);
        println!("grayscale={}", self.grayscale);
        println!("stride={}", self.stride);
        println!("memory_bytes={}", self.memory_bytes);
        println!("rotate={}", self.rotate);
        println!("red={}", self.red);
        println!("green={}", self.green);
        println!("blue={}", self.blue);
        println!("transp={}", self.transp);
    }
}

pub fn inspect(path: &Path) -> io::Result<FramebufferInfo> {
    let file = File::open(path)?;
    let var = query_var(&file)?;
    let fix = query_fix(&file)?;
    validate(&var, &fix)?;
    let driver = read_driver_name();
    Ok(FramebufferInfo {
        device: path.to_path_buf(),
        driver,
        width: var.xres,
        height: var.yres,
        virtual_width: var.xres_virtual,
        virtual_height: var.yres_virtual,
        xoffset: var.xoffset,
        yoffset: var.yoffset,
        bits_per_pixel: var.bits_per_pixel,
        grayscale: var.grayscale,
        stride: fix.line_length,
        memory_bytes: fix.smem_len,
        rotate: var.rotate,
        red: channel(var.red),
        green: channel(var.green),
        blue: channel(var.blue),
        transp: channel(var.transp),
    })
}

pub fn capture_to(path: &Path, output: &mut impl Write) -> io::Result<FramebufferInfo> {
    let file = File::open(path)?;
    let var = query_var(&file)?;
    let fix = query_fix(&file)?;
    validate(&var, &fix)?;
    let info = FramebufferInfo {
        device: path.to_path_buf(),
        driver: read_driver_name(),
        width: var.xres,
        height: var.yres,
        virtual_width: var.xres_virtual,
        virtual_height: var.yres_virtual,
        xoffset: var.xoffset,
        yoffset: var.yoffset,
        bits_per_pixel: var.bits_per_pixel,
        grayscale: var.grayscale,
        stride: fix.line_length,
        memory_bytes: fix.smem_len,
        rotate: var.rotate,
        red: channel(var.red),
        green: channel(var.green),
        blue: channel(var.blue),
        transp: channel(var.transp),
    };

    let map_len = usize::try_from(fix.smem_len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "framebuffer memory size does not fit in usize",
        )
    })?;
    let mapping = MappedFramebuffer::new(&file, map_len)?;
    write_pgm(&var, &fix, mapping.as_slice(), output)?;
    Ok(info)
}

fn write_pgm(
    var: &FbVarScreeninfo,
    fix: &FbFixScreeninfo,
    mapped: &[u8],
    output: &mut impl Write,
) -> io::Result<()> {
    writeln!(output, "P5")?;
    writeln!(output, "{} {}", var.xres, var.yres)?;
    writeln!(output, "255")?;

    let bytes_per_pixel = usize::try_from(var.bits_per_pixel.div_ceil(8)).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "invalid framebuffer pixel size")
    })?;
    let stride = usize::try_from(fix.line_length)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid framebuffer stride"))?;
    let xoffset = usize::try_from(var.xoffset)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid framebuffer x offset"))?;
    let yoffset = usize::try_from(var.yoffset)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid framebuffer y offset"))?;
    let width = usize::try_from(var.xres)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid framebuffer width"))?;
    let height = usize::try_from(var.yres)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid framebuffer height"))?;
    let mut row = vec![0u8; width];
    for y in 0..height {
        let row_start = yoffset
            .checked_add(y)
            .and_then(|row| row.checked_mul(stride))
            .and_then(|offset| offset.checked_add(xoffset.checked_mul(bytes_per_pixel)?))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "framebuffer offset overflow")
            })?;
        let row_bytes = width.checked_mul(bytes_per_pixel).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "framebuffer row overflow")
        })?;
        let row_end = row_start.checked_add(row_bytes).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "framebuffer row overflow")
        })?;
        let source = mapped.get(row_start..row_end).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "visible framebuffer exceeds mapped memory",
            )
        })?;
        for (x, pixel) in source.chunks_exact(bytes_per_pixel).enumerate() {
            row[x] = pixel_to_gray(pixel, &var);
        }
        output.write_all(&row)?;
    }
    output.flush()?;
    Ok(())
}

/// Render a centered marker, exercise the T1's EPDC update ABI, capture the
/// resulting framebuffer memory, and restore the bytes that were changed.
///
/// This is deliberately bounded and does not stop or signal any Android
/// process. The capture proves that our native process wrote the expected
/// pixels into the mapped framebuffer; it cannot by itself prove what the
/// e-ink panel displayed, so the caller should observe the reader during the
/// wait interval as well.
pub fn render_test_to(
    path: &Path,
    wait_after_capture: Duration,
    waveform: WaveformMode,
    wait_for_completion: bool,
    output: &mut impl Write,
) -> io::Result<()> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    let var = query_var(&file)?;
    let fix = query_fix(&file)?;
    validate(&var, &fix)?;
    if var.bits_per_pixel != 16
        || var.red.offset != 11
        || var.red.length != 5
        || var.green.offset != 5
        || var.green.length != 6
        || var.blue.offset != 0
        || var.blue.length != 5
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "render-test requires the T1 RGB565 framebuffer format",
        ));
    }

    let map_len = usize::try_from(fix.smem_len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "framebuffer memory size does not fit in usize",
        )
    })?;
    let mapping = MappedFramebuffer::new_with_protection(&file, map_len, PROT_READ | PROT_WRITE)?;
    let rect = MxcfbRect {
        top: (var.yres.saturating_sub(TEST_RECT_HEIGHT)) / 2,
        left: (var.xres.saturating_sub(TEST_RECT_WIDTH)) / 2,
        width: TEST_RECT_WIDTH.min(var.xres),
        height: TEST_RECT_HEIGHT.min(var.yres),
    };
    let layout = FramebufferLayout::new(&var, &fix, rect)?;
    let backup = layout.copy_visible(mapping.as_slice())?;
    layout.draw_marker(mapping.as_mut_slice())?;
    let marker = layout.copy_visible(mapping.as_slice())?;

    eprintln!(
        "render-test: marker rect left={} top={} width={} height={}",
        rect.left, rect.top, rect.width, rect.height
    );
    eprintln!("render-test: waveform={}", waveform.label());
    let started = Instant::now();
    let update_result = if wait_for_completion {
        request_update(file.as_raw_fd(), rect, waveform, TEST_UPDATE_MARKER)
    } else {
        submit_update(file.as_raw_fd(), rect, waveform, TEST_UPDATE_MARKER, false)
    };
    if let Err(error) = update_result {
        layout.restore(mapping.as_mut_slice(), &backup)?;
        return Err(error);
    }
    eprintln!(
        "render-test: {}_elapsed_ms={} status=ok",
        if wait_for_completion {
            "update"
        } else {
            "update_submission"
        },
        started.elapsed().as_millis(),
    );

    let capture_result = write_pgm(&var, &fix, mapping.as_slice(), output);
    let changed_before_wait = layout.differs_from(mapping.as_slice(), &backup);
    thread::sleep(wait_after_capture);
    let changed_after_wait = layout.differs_from(mapping.as_slice(), &backup);
    let marker_preserved_after_wait = layout.matches(mapping.as_slice(), &marker);

    let restore_result = layout.restore(mapping.as_mut_slice(), &backup);
    let restore_update_result = request_update(
        file.as_raw_fd(),
        rect,
        WaveformMode::Gc16,
        TEST_UPDATE_MARKER + 1,
    );

    if let Err(error) = restore_result {
        return Err(error);
    }
    restore_update_result?;
    eprintln!(
        "render-test: framebuffer_marker_changed_before_wait={}",
        changed_before_wait?
    );
    eprintln!(
        "render-test: framebuffer_marker_changed_after_wait={}",
        changed_after_wait?
    );
    eprintln!(
        "render-test: exact_marker_preserved_after_wait={}",
        marker_preserved_after_wait?
    );
    eprintln!("render-test: original rectangle restored and refreshed");
    capture_result
}

fn request_update(
    fd: c_int,
    rect: MxcfbRect,
    waveform: WaveformMode,
    marker: u32,
) -> io::Result<()> {
    submit_update(fd, rect, waveform, marker, false)?;
    wait_for_update_complete(fd, marker)
}

fn submit_update(
    fd: c_int,
    rect: MxcfbRect,
    waveform: WaveformMode,
    marker: u32,
    full_update: bool,
) -> io::Result<()> {
    let mut auto_update_mode = AUTO_UPDATE_MODE_REGION;
    let result = unsafe {
        ioctl(
            fd,
            MXCFB_SET_AUTO_UPDATE_MODE,
            &mut auto_update_mode as *mut u32,
        )
    };
    if result < 0 {
        return Err(ioctl_error("set auto-update mode"));
    }

    let mut update = MxcfbUpdateData {
        update_region: rect,
        waveform_mode: waveform.value(),
        update_mode: update_mode(full_update),
        update_marker: marker,
        temperature: TEMP_USE_AMBIENT,
        ..Default::default()
    };
    debug_assert_eq!(std::mem::size_of::<MxcfbUpdateData>(), 0x44);
    let result = unsafe { ioctl(fd, MXCFB_SEND_UPDATE, &mut update as *mut MxcfbUpdateData) };
    if result < 0 {
        return Err(ioctl_error("send display update"));
    }
    Ok(())
}

fn update_mode(full_update: bool) -> u32 {
    if full_update {
        UPDATE_MODE_FULL
    } else {
        UPDATE_MODE_PARTIAL
    }
}

fn wait_for_update_complete(fd: c_int, marker: u32) -> io::Result<()> {
    let mut completed_marker = marker;
    let result = unsafe {
        ioctl(
            fd,
            MXCFB_WAIT_FOR_UPDATE_COMPLETE,
            &mut completed_marker as *mut u32,
        )
    };
    if result < 0 {
        return Err(ioctl_error("wait for display update"));
    }
    Ok(())
}

fn ioctl_error(stage: &str) -> io::Error {
    let error = io::Error::last_os_error();
    io::Error::new(error.kind(), format!("{stage}: {error}"))
}

fn query_var(file: &File) -> io::Result<FbVarScreeninfo> {
    let mut value = FbVarScreeninfo::default();
    let result = unsafe { ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO, &mut value) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}

fn query_fix(file: &File) -> io::Result<FbFixScreeninfo> {
    let mut value = FbFixScreeninfo::default();
    let result = unsafe { ioctl(file.as_raw_fd(), FBIOGET_FSCREENINFO, &mut value) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}

fn validate(var: &FbVarScreeninfo, fix: &FbFixScreeninfo) -> io::Result<()> {
    if var.xres == 0 || var.yres == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "framebuffer has zero visible dimensions",
        ));
    }
    if !(8..=32).contains(&var.bits_per_pixel) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported framebuffer depth: {}", var.bits_per_pixel),
        ));
    }
    if fix.line_length == 0 || fix.smem_len == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "framebuffer has zero stride or memory size",
        ));
    }
    if usize::try_from(fix.smem_len).unwrap_or(usize::MAX) > MAX_MAPPED_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("framebuffer memory exceeds {MAX_MAPPED_BYTES} bytes"),
        ));
    }
    Ok(())
}

fn channel(field: FbBitfield) -> ChannelInfo {
    ChannelInfo {
        offset: field.offset,
        length: field.length,
    }
}

fn pixel_to_gray(pixel: &[u8], var: &FbVarScreeninfo) -> u8 {
    let mut bytes = [0u8; 4];
    bytes[..pixel.len()].copy_from_slice(pixel);
    let value = u32::from_ne_bytes(bytes);
    let fields_present = var.red.length != 0 || var.green.length != 0 || var.blue.length != 0;
    if !fields_present {
        let max = if var.bits_per_pixel == 32 {
            u32::MAX
        } else {
            (1u32 << var.bits_per_pixel) - 1
        };
        return scale_to_byte(value & max, max);
    }

    let red = field_to_byte(value, var.red);
    let green = field_to_byte(value, var.green);
    let blue = field_to_byte(value, var.blue);
    ((u32::from(red) * 77 + u32::from(green) * 150 + u32::from(blue) * 29 + 128) / 256) as u8
}

fn field_to_byte(value: u32, field: FbBitfield) -> u8 {
    if field.length == 0 || field.offset >= 32 {
        return 0;
    }
    let available = 32 - field.offset;
    let length = field.length.min(available);
    let mask = if length == 32 {
        u32::MAX
    } else {
        (1u32 << length) - 1
    };
    scale_to_byte((value >> field.offset) & mask, mask)
}

fn scale_to_byte(value: u32, max: u32) -> u8 {
    if max == 0 {
        0
    } else {
        ((value * 255 + max / 2) / max) as u8
    }
}

fn read_driver_name() -> String {
    std::fs::read_to_string("/proc/fb")
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                line.split_once(' ')
                    .map(|(_, name)| name.trim().to_string())
            })
        })
        .unwrap_or_else(|| "unknown".into())
}

struct MappedFramebuffer {
    address: *mut c_void,
    length: usize,
}

struct PendingUpdate {
    marker: u32,
    region: DisplayRegion,
    pixels: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisplayUpdateRequest {
    region: DisplayRegion,
    waveform: WaveformMode,
    force_refresh: bool,
}

impl MappedFramebuffer {
    fn new(file: &File, length: usize) -> io::Result<Self> {
        Self::new_with_protection(file, length, PROT_READ)
    }

    fn new_with_protection(file: &File, length: usize, protection: c_int) -> io::Result<Self> {
        let address = unsafe {
            mmap(
                ptr::null_mut(),
                length,
                protection,
                MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if address == MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { address, length })
    }

    fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.address.cast::<u8>(), self.length) }
    }

    fn as_mut_slice(&self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.address.cast::<u8>(), self.length) }
    }
}

/// A write-capable T1 framebuffer session for the native runtime.
pub struct NativeDisplay {
    file: File,
    var: FbVarScreeninfo,
    fix: FbFixScreeninfo,
    mapping: Option<MappedFramebuffer>,
    next_marker: u32,
    pending_update: Option<PendingUpdate>,
    presented: Option<Vec<u8>>,
    orientation: ReaderOrientation,
}

impl NativeDisplay {
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::open_with_orientation(path, ReaderOrientation::Portrait)
    }

    pub fn open_with_orientation(path: &Path, orientation: ReaderOrientation) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut var = query_var(&file)?;
        let mut fix = query_fix(&file)?;
        validate(&var, &fix)?;
        validate_rgb565(&var)?;
        // The T1 driver can inherit Android's landscape mode after handoff.
        // Keep the first writable mmap before changing any other startup state;
        // this ordering avoids the device's known second-mmap EINVAL path.
        let mapping = MappedFramebuffer::new_with_protection(
            &file,
            map_length(&fix)?,
            PROT_READ | PROT_WRITE,
        )?;
        establish_native_orientation(&file, &mut var, &mut fix, orientation, Some(mapping.length))?;
        Ok(Self {
            file,
            var,
            fix,
            mapping: Some(mapping),
            next_marker: 10,
            pending_update: None,
            presented: None,
            orientation,
        })
    }

    pub fn width(&self) -> u32 {
        self.var.xres
    }

    pub fn height(&self) -> u32 {
        self.var.yres
    }

    pub fn orientation(&self) -> ReaderOrientation {
        self.orientation
    }

    /// Change the physical fbdev orientation while retaining the validated
    /// mapping. The next render is forced to establish a complete shadow for
    /// the newly oriented panel.
    pub fn set_orientation(&mut self, orientation: ReaderOrientation) -> io::Result<()> {
        if self.orientation == orientation && native_orientation_matches(&self.var, orientation) {
            return Ok(());
        }
        self.wait_for_pending_update()?;
        establish_native_orientation(
            &self.file,
            &mut self.var,
            &mut self.fix,
            orientation,
            self.mapping.as_ref().map(|mapping| mapping.length),
        )?;
        self.orientation = orientation;
        self.presented = None;
        Ok(())
    }

    /// Compare an owned packed RGB565 frame with the last completed frame,
    /// copy only the changed rectangle into the mapped framebuffer, and
    /// submit that rectangle to the EPDC.
    ///
    /// The semantic `requested_region` remains useful as a waveform hint and
    /// as the fallback when no completed shadow exists. The actual update
    /// geometry comes from the pixel diff, so unchanged pixels in a semantic
    /// region are no longer refreshed. The mapped framebuffer is never
    /// modified while an earlier asynchronous update is outstanding.
    pub fn draw_frame_with_waveform(
        &mut self,
        frame: &[u8],
        requested_region: DisplayRegion,
        waveform: WaveformMode,
        wait_for_completion: bool,
        force_refresh: bool,
    ) -> io::Result<()> {
        let width = usize::try_from(self.var.xres)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid display width"))?;
        let height = usize::try_from(self.var.yres)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid display height"))?;
        let expected = packed_frame_len(width, height)?;
        if frame.len() != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "display frame is {} bytes, expected {expected}",
                    frame.len()
                ),
            ));
        }
        let requested_region = requested_region.bounded(self.var.xres, self.var.yres);
        if requested_region.width == 0 || requested_region.height == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "display update region is empty",
            ));
        }

        // One outstanding update is the safe baseline until we have a display
        // worker with an immutable queue of submitted regions. Rendering can
        // happen ahead of this barrier because `frame` is owned by the caller.
        self.wait_for_pending_update()?;

        let Some(request) = display_update_request(
            frame,
            self.presented.as_deref(),
            requested_region,
            waveform,
            force_refresh,
            self.var.xres,
            self.var.yres,
        )?
        else {
            eprintln!(
                "standalone-test: display refresh skipped unchanged requested=({},{} {}x{})",
                requested_region.left,
                requested_region.top,
                requested_region.width,
                requested_region.height,
            );
            return Ok(());
        };
        let pixels = self.copy_frame_region(frame, request.region)?;

        let marker = self.next_marker;
        self.next_marker = self.next_marker.wrapping_add(1).max(10);
        let started = Instant::now();
        submit_update(
            self.file.as_raw_fd(),
            request.region.as_mxcfb(),
            request.waveform,
            marker,
            request.force_refresh,
        )?;
        let pending = PendingUpdate {
            marker,
            region: request.region,
            pixels,
        };
        if wait_for_completion {
            match wait_for_update_complete(self.file.as_raw_fd(), marker) {
                Ok(()) => {
                    if let Err(error) = self.commit_update(&pending) {
                        self.pending_update = Some(pending);
                        return Err(error);
                    }
                }
                Err(error) => {
                    self.pending_update = Some(pending);
                    return Err(error);
                }
            }
        } else {
            self.pending_update = Some(pending);
        }
        eprintln!(
            "standalone-test: display refresh region=({},{} {}x{}) waveform={} update_mode={} completion={} elapsed_ms={} status={}",
            request.region.left,
            request.region.top,
            request.region.width,
            request.region.height,
            request.waveform.label(),
            if request.force_refresh { "FULL" } else { "PARTIAL" },
            if wait_for_completion { "wait" } else { "nowait" },
            started.elapsed().as_millis(),
            "ok",
        );
        Ok(())
    }

    fn wait_for_pending_update(&mut self) -> io::Result<()> {
        let Some(pending) = self.pending_update.take() else {
            return Ok(());
        };
        let marker = pending.marker;
        let started = Instant::now();
        if let Err(error) = wait_for_update_complete(self.file.as_raw_fd(), marker) {
            self.pending_update = Some(pending);
            return Err(error);
        }
        if let Err(error) = self.commit_update(&pending) {
            self.pending_update = Some(pending);
            return Err(error);
        }
        eprintln!(
            "standalone-test: waited for pending display marker={} elapsed_ms={}",
            marker,
            started.elapsed().as_millis(),
        );
        Ok(())
    }

    fn copy_frame_region(&mut self, frame: &[u8], region: DisplayRegion) -> io::Result<Vec<u8>> {
        let width = usize::try_from(self.var.xres)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid display width"))?;
        let stride = usize::try_from(self.fix.line_length)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid display stride"))?;
        let xoffset = usize::try_from(self.var.xoffset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid display x offset"))?;
        let yoffset = usize::try_from(self.var.yoffset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid display y offset"))?;
        let region_width = usize::try_from(region.width)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid update width"))?;
        let region_height = usize::try_from(region.height)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid update height"))?;
        let row_bytes = region_width
            .checked_mul(2)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "update row overflow"))?;
        let mut pixels =
            Vec::with_capacity(row_bytes.checked_mul(region_height).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "update size overflow")
            })?);
        let mapping = self.mapping.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "framebuffer is not mapped")
        })?;
        for row in 0..region_height {
            let source_row = usize::try_from(region.top)
                .unwrap()
                .checked_add(row)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "update row overflow"))?;
            let source_column = usize::try_from(region.left).unwrap();
            let source_start = source_row
                .checked_mul(width)
                .and_then(|offset| offset.checked_add(source_column))
                .and_then(|offset| offset.checked_mul(2))
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "update offset overflow")
                })?;
            let source_end = source_start
                .checked_add(row_bytes)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "update row overflow"))?;
            let source = frame.get(source_start..source_end).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "update exceeds packed frame")
            })?;
            pixels.extend_from_slice(source);

            let target_start = yoffset
                .checked_add(source_row)
                .and_then(|offset| offset.checked_mul(stride))
                .and_then(|offset| {
                    offset.checked_add(xoffset.checked_add(source_column)?.checked_mul(2)?)
                })
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "update offset overflow")
                })?;
            let target_end = target_start
                .checked_add(row_bytes)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "update row overflow"))?;
            if target_end > mapping.length {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "update exceeds mapped framebuffer",
                ));
            }
            mapping.as_mut_slice()[target_start..target_end].copy_from_slice(source);
        }
        Ok(pixels)
    }

    fn commit_update(&mut self, pending: &PendingUpdate) -> io::Result<()> {
        let width = self.var.xres;
        let height = self.var.yres;
        let full = DisplayRegion::full(width, height);
        if self.presented.is_none() {
            if pending.region != full {
                // A partial update cannot establish pixels outside its own
                // rectangle. Keep the shadow invalid until a full sync.
                return Ok(());
            }
            self.presented = Some(vec![0; packed_frame_len_u32(width, height)?]);
        }
        copy_packed_region(
            self.presented
                .as_mut()
                .expect("presented initialized above"),
            width,
            pending.region,
            &pending.pixels,
        )
    }

    /// Copy a logical RGB565 screen into the EPDC driver's hidden standby
    /// buffer. The kernel supplies this image during early suspend.
    pub fn write_standby(&self, image: &[u8]) -> io::Result<()> {
        let width = usize::try_from(self.var.xres)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid display width"))?;
        let height = usize::try_from(self.var.yres)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid display height"))?;
        let expected = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "standby image is too large")
            })?;
        if image.len() != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "standby image is {} bytes, expected {expected}",
                    image.len()
                ),
            ));
        }

        let result = unsafe { ioctl(self.file.as_raw_fd(), MXCFB_WRITE_SSCREEN, image.as_ptr()) };
        if result < 0 {
            return Err(ioctl_error("write standby screen"));
        }
        Ok(())
    }

    pub fn prepare_for_suspend(&mut self) {
        // The T1's EPDC driver keeps the DMA framebuffer allocated while the
        // panel is suspended, but rejects a second mmap after resume. Retain
        // this mapping and refresh only the framebuffer metadata on wake.
    }

    pub fn resume_after_suspend(&mut self) -> io::Result<()> {
        let mut last_error = None;
        for attempt in 0..20 {
            match self.try_resume_after_suspend() {
                Ok(()) => {
                    if attempt != 0 {
                        eprintln!(
                            "standalone-test: framebuffer remap succeeded after {} retries",
                            attempt
                        );
                    }
                    return Ok(());
                }
                Err(error) => {
                    eprintln!(
                        "standalone-test: framebuffer remap attempt {} failed: {error}",
                        attempt + 1
                    );
                    if error.raw_os_error() != Some(22) || attempt == 19 {
                        return Err(error);
                    }
                    last_error = Some(error);
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "framebuffer remap retries exhausted")
        }))
    }

    fn try_resume_after_suspend(&mut self) -> io::Result<()> {
        self.var = query_var(&self.file)?;
        self.fix = query_fix(&self.file)?;
        eprintln!(
            "standalone-test: post-resume framebuffer {}x{} virtual={}x{} smem_len={} stride={} offsets=({}, {})",
            self.var.xres,
            self.var.yres,
            self.var.xres_virtual,
            self.var.yres_virtual,
            self.fix.smem_len,
            self.fix.line_length,
            self.var.xoffset,
            self.var.yoffset,
        );
        establish_native_orientation(
            &self.file,
            &mut self.var,
            &mut self.fix,
            self.orientation,
            self.mapping.as_ref().map(|mapping| mapping.length),
        )?;
        if self.mapping.is_none() {
            self.mapping = Some(MappedFramebuffer::new_with_protection(
                &self.file,
                map_length(&self.fix)?,
                PROT_READ | PROT_WRITE,
            )?);
            eprintln!("standalone-test: framebuffer mapping created after resume");
        } else {
            eprintln!("standalone-test: framebuffer mapping retained across resume");
        }
        // The panel's visible contents after suspend are not represented by
        // the retained mmap contents. Require a complete synchronization
        // before accepting another incremental update.
        self.presented = None;
        Ok(())
    }
}

fn display_update_request(
    frame: &[u8],
    presented: Option<&[u8]>,
    requested_region: DisplayRegion,
    waveform: WaveformMode,
    force_refresh: bool,
    width: u32,
    height: u32,
) -> io::Result<Option<DisplayUpdateRequest>> {
    let changed = if force_refresh || presented.is_none() {
        Some(requested_region)
    } else {
        damage::changed_region(
            frame,
            presented.expect("presented checked above"),
            width,
            height,
            DamageOptions::default(),
        )?
    };
    let Some(region) = changed else {
        return Ok(None);
    };
    Ok(Some(DisplayUpdateRequest {
        region,
        waveform: if requested_region.contains(region) {
            waveform
        } else {
            // An unexpected change escaped the semantic hint. Use the safer
            // waveform until the runtime has an explicit damage set.
            WaveformMode::Gc16
        },
        force_refresh,
    }))
}

pub struct DisplayCanvas<'a> {
    buffer: &'a mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    xoffset: usize,
    yoffset: usize,
}

impl DisplayCanvas<'_> {
    pub(crate) fn new(
        buffer: &mut [u8],
        width: usize,
        height: usize,
        stride: usize,
        xoffset: usize,
        yoffset: usize,
    ) -> DisplayCanvas<'_> {
        DisplayCanvas {
            buffer,
            width,
            height,
            stride,
            xoffset,
            yoffset,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn fill(&mut self, pixel: u16) {
        for y in 0..self.height {
            for x in 0..self.width {
                self.set_pixel(x, y, pixel);
            }
        }
    }

    pub fn set_pixel(&mut self, x: usize, y: usize, pixel: u16) {
        if x >= self.width || y >= self.height {
            return;
        }
        let Some(offset) = self
            .yoffset
            .checked_add(y)
            .and_then(|row| row.checked_mul(self.stride))
            .and_then(|offset| {
                self.xoffset
                    .checked_add(x)
                    .and_then(|column| column.checked_mul(2))
                    .and_then(|column| offset.checked_add(column))
            })
        else {
            return;
        };
        let Some(target) = self.buffer.get_mut(offset..offset.saturating_add(2)) else {
            return;
        };
        if target.len() == 2 {
            target.copy_from_slice(&pixel.to_ne_bytes());
        }
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, pixel: u16) {
        let x_end = x.saturating_add(width).min(self.width);
        let y_end = y.saturating_add(height).min(self.height);
        for row in y..y_end {
            for column in x..x_end {
                self.set_pixel(column, row, pixel);
            }
        }
    }

    pub fn stroke_rect(&mut self, x: usize, y: usize, width: usize, height: usize, pixel: u16) {
        if width == 0 || height == 0 {
            return;
        }
        for column in x..x.saturating_add(width) {
            self.set_pixel(column, y, pixel);
            self.set_pixel(column, y.saturating_add(height - 1), pixel);
        }
        for row in y..y.saturating_add(height) {
            self.set_pixel(x, row, pixel);
            self.set_pixel(x.saturating_add(width - 1), row, pixel);
        }
    }
}

fn packed_frame_len(width: usize, height: usize) -> io::Result<usize> {
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(2))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "display frame is too large"))
}

fn packed_frame_len_u32(width: u32, height: u32) -> io::Result<usize> {
    packed_frame_len(
        usize::try_from(width)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid display width"))?,
        usize::try_from(height)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid display height"))?,
    )
}

fn copy_packed_region(
    target: &mut [u8],
    width: u32,
    region: DisplayRegion,
    pixels: &[u8],
) -> io::Result<()> {
    let width = usize::try_from(width)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid display width"))?;
    let region_width = usize::try_from(region.width)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid update width"))?;
    let region_height = usize::try_from(region.height)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid update height"))?;
    let row_bytes = region_width
        .checked_mul(2)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "update row overflow"))?;
    let expected = row_bytes
        .checked_mul(region_height)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "update size overflow"))?;
    if pixels.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "update snapshot is {} bytes, expected {expected}",
                pixels.len()
            ),
        ));
    }
    let full_row_bytes = width
        .checked_mul(2)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "display row overflow"))?;
    for row in 0..region_height {
        let source_start = row * row_bytes;
        let target_start = (usize::try_from(region.top).unwrap() + row)
            .checked_mul(full_row_bytes)
            .and_then(|offset| {
                offset.checked_add(usize::try_from(region.left).unwrap().checked_mul(2)?)
            })
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "shadow offset overflow"))?;
        let target_end = target_start
            .checked_add(row_bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "shadow row overflow"))?;
        if target_end > target.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "update exceeds packed shadow",
            ));
        }
        target[target_start..target_end]
            .copy_from_slice(&pixels[source_start..source_start + row_bytes]);
    }
    Ok(())
}

fn map_length(fix: &FbFixScreeninfo) -> io::Result<usize> {
    usize::try_from(fix.smem_len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "framebuffer memory size does not fit in usize",
        )
    })
}

fn establish_native_orientation(
    file: &File,
    var: &mut FbVarScreeninfo,
    fix: &mut FbFixScreeninfo,
    orientation: ReaderOrientation,
    mapped_length: Option<usize>,
) -> io::Result<()> {
    if !native_orientation_matches(var, orientation) {
        var.rotate = orientation.framebuffer_rotation();
        let result = unsafe {
            ioctl(
                file.as_raw_fd(),
                FBIOPUT_VSCREENINFO,
                var as *mut FbVarScreeninfo,
            )
        };
        if result < 0 {
            return Err(ioctl_error("set native framebuffer orientation"));
        }
    }

    // The driver derives xres/yres and the fixed stride from the requested
    // rotation. Use its returned values for every subsequent render and
    // update, instead of assuming that the ioctl accepted the request.
    *var = query_var(file)?;
    *fix = query_fix(file)?;
    validate(var, fix)?;
    validate_rgb565(var)?;
    validate_native_orientation(var, orientation)?;
    if let Some(mapped_length) = mapped_length {
        let required_length = map_length(fix)?;
        if required_length > mapped_length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "native framebuffer mapping is too small after orientation: {mapped_length} < {required_length}"
                ),
            ));
        }
    }
    Ok(())
}

fn native_orientation_matches(var: &FbVarScreeninfo, orientation: ReaderOrientation) -> bool {
    let viewport = orientation.viewport();
    var.xres == viewport.width
        && var.yres == viewport.height
        && var.rotate == orientation.framebuffer_rotation()
}

fn validate_native_orientation(
    var: &FbVarScreeninfo,
    orientation: ReaderOrientation,
) -> io::Result<()> {
    if native_orientation_matches(var, orientation) {
        return Ok(());
    }
    let viewport = orientation.viewport();
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "native {} display requires {}x{} at rotation {}, got {}x{} at rotation {}",
            orientation.label(),
            viewport.width,
            viewport.height,
            orientation.framebuffer_rotation(),
            var.xres,
            var.yres,
            var.rotate,
        ),
    ))
}

fn validate_rgb565(var: &FbVarScreeninfo) -> io::Result<()> {
    if var.bits_per_pixel != 16
        || var.red.offset != 11
        || var.red.length != 5
        || var.green.offset != 5
        || var.green.length != 6
        || var.blue.offset != 0
        || var.blue.length != 5
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native display requires the T1 RGB565 framebuffer format",
        ));
    }
    Ok(())
}

impl Drop for MappedFramebuffer {
    fn drop(&mut self) {
        let _ = unsafe { munmap(self.address, self.length) };
    }
}

struct FramebufferLayout {
    rect: MxcfbRect,
    stride: usize,
    xoffset: usize,
    yoffset: usize,
}

impl FramebufferLayout {
    fn new(var: &FbVarScreeninfo, fix: &FbFixScreeninfo, rect: MxcfbRect) -> io::Result<Self> {
        if rect.width == 0
            || rect.height == 0
            || rect.left.checked_add(rect.width).is_none()
            || rect.top.checked_add(rect.height).is_none()
            || rect.left + rect.width > var.xres
            || rect.top + rect.height > var.yres
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "render rectangle is outside the visible framebuffer",
            ));
        }
        Ok(Self {
            rect,
            stride: usize::try_from(fix.line_length).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid framebuffer stride")
            })?,
            xoffset: usize::try_from(var.xoffset).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid framebuffer x offset")
            })?,
            yoffset: usize::try_from(var.yoffset).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid framebuffer y offset")
            })?,
        })
    }

    fn row_start(&self, y: u32) -> io::Result<usize> {
        let row = self
            .yoffset
            .checked_add(usize::try_from(self.rect.top + y).unwrap_or(usize::MAX))
            .and_then(|row| row.checked_mul(self.stride))
            .and_then(|offset| {
                offset.checked_add(
                    self.xoffset
                        .checked_add(usize::try_from(self.rect.left).unwrap_or(usize::MAX))?
                        .checked_mul(2)?,
                )
            })
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "render offset overflow"))?;
        Ok(row)
    }

    fn row_range(
        &self,
        y: u32,
        length: usize,
        buffer_len: usize,
    ) -> io::Result<std::ops::Range<usize>> {
        let start = self.row_start(y)?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "render row overflow"))?;
        if end > buffer_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "render rectangle exceeds mapped framebuffer",
            ));
        }
        Ok(start..end)
    }

    fn copy_visible(&self, buffer: &[u8]) -> io::Result<Vec<u8>> {
        let row_bytes = usize::try_from(self.rect.width)
            .ok()
            .and_then(|width| width.checked_mul(2))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "render row overflow"))?;
        let mut backup = Vec::with_capacity(
            row_bytes
                .checked_mul(usize::try_from(self.rect.height).unwrap_or(usize::MAX))
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "render backup overflow")
                })?,
        );
        for y in 0..self.rect.height {
            backup.extend_from_slice(&buffer[self.row_range(y, row_bytes, buffer.len())?]);
        }
        Ok(backup)
    }

    fn restore(&self, buffer: &mut [u8], backup: &[u8]) -> io::Result<()> {
        let row_bytes = usize::try_from(self.rect.width)
            .ok()
            .and_then(|width| width.checked_mul(2))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "render row overflow"))?;
        let expected = row_bytes
            .checked_mul(usize::try_from(self.rect.height).unwrap_or(usize::MAX))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "render backup overflow"))?;
        if backup.len() != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "render backup has an unexpected size",
            ));
        }
        for y in 0..self.rect.height {
            let source_start = usize::try_from(y)
                .ok()
                .and_then(|y| y.checked_mul(row_bytes))
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "render backup overflow")
                })?;
            let range = self.row_range(y, row_bytes, buffer.len())?;
            buffer[range].copy_from_slice(&backup[source_start..source_start + row_bytes]);
        }
        Ok(())
    }

    fn differs_from(&self, buffer: &[u8], backup: &[u8]) -> io::Result<bool> {
        let current = self.copy_visible(buffer)?;
        Ok(current != backup)
    }

    fn matches(&self, buffer: &[u8], expected: &[u8]) -> io::Result<bool> {
        let current = self.copy_visible(buffer)?;
        Ok(current == expected)
    }

    fn draw_marker(&self, buffer: &mut [u8]) -> io::Result<()> {
        for y in 0..self.rect.height {
            let range = self.row_range(
                y,
                usize::try_from(self.rect.width).unwrap() * 2,
                buffer.len(),
            )?;
            let row = &mut buffer[range];
            for x in 0..self.rect.width {
                let border =
                    x < 4 || y < 4 || x + 4 >= self.rect.width || y + 4 >= self.rect.height;
                let diagonal = x * self.rect.height / self.rect.width == y
                    || (self.rect.width - 1 - x) * self.rect.height / self.rect.width == y;
                let cross = x == self.rect.width / 2 || y == self.rect.height / 2;
                let pixel = if border || diagonal || cross {
                    0x0000u16
                } else {
                    0xffffu16
                };
                let bytes = pixel.to_ne_bytes();
                let offset = usize::try_from(x).unwrap() * 2;
                row[offset..offset + 2].copy_from_slice(&bytes);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb565_var() -> FbVarScreeninfo {
        FbVarScreeninfo {
            bits_per_pixel: 16,
            red: FbBitfield {
                offset: 11,
                length: 5,
                ..Default::default()
            },
            green: FbBitfield {
                offset: 5,
                length: 6,
                ..Default::default()
            },
            blue: FbBitfield {
                offset: 0,
                length: 5,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn converts_rgb565_channels_to_gray() {
        let var = rgb565_var();
        assert_eq!(pixel_to_gray(&[0x00, 0xf8], &var), 77);
        assert_eq!(pixel_to_gray(&[0xe0, 0x07], &var), 149);
        assert_eq!(pixel_to_gray(&[0x1f, 0x00], &var), 29);
    }

    #[test]
    fn scales_grayscale_pixels_without_bitfields() {
        let var = FbVarScreeninfo {
            bits_per_pixel: 8,
            ..Default::default()
        };
        assert_eq!(pixel_to_gray(&[0x00], &var), 0);
        assert_eq!(pixel_to_gray(&[0x80], &var), 128);
        assert_eq!(pixel_to_gray(&[0xff], &var), 255);
    }

    #[test]
    fn t1_update_payload_matches_vendor_ioctl_size() {
        assert_eq!(std::mem::size_of::<MxcfbUpdateData>(), 0x44);
        assert_eq!(MXCFB_SEND_UPDATE, 0x4044_462e);
    }

    #[test]
    fn native_orientation_requires_portrait_framebuffer_and_rotation_three() {
        let portrait = FbVarScreeninfo {
            xres: NATIVE_DISPLAY_WIDTH,
            yres: NATIVE_DISPLAY_HEIGHT,
            rotate: ReaderOrientation::Portrait.framebuffer_rotation(),
            ..rgb565_var()
        };
        assert!(native_orientation_matches(
            &portrait,
            ReaderOrientation::Portrait
        ));
        assert!(validate_native_orientation(&portrait, ReaderOrientation::Portrait).is_ok());

        let landscape = FbVarScreeninfo {
            xres: LANDSCAPE_DISPLAY_WIDTH,
            yres: LANDSCAPE_DISPLAY_HEIGHT,
            rotate: 0,
            ..portrait
        };
        assert!(native_orientation_matches(
            &landscape,
            ReaderOrientation::Landscape
        ));
        assert!(validate_native_orientation(&landscape, ReaderOrientation::Landscape).is_ok());
        assert!(!native_orientation_matches(
            &landscape,
            ReaderOrientation::Portrait
        ));
        assert!(validate_native_orientation(&landscape, ReaderOrientation::Portrait).is_err());
    }

    #[test]
    fn framebuffer_orientation_uses_fbdev_put_vscreeninfo() {
        assert_eq!(FBIOPUT_VSCREENINFO, 0x4601);
        assert_eq!(ReaderOrientation::Portrait.framebuffer_rotation(), 3);
        assert_eq!(ReaderOrientation::Landscape.framebuffer_rotation(), 0);
    }

    #[test]
    fn forced_refresh_uses_full_epdc_update_mode() {
        assert_eq!(update_mode(false), UPDATE_MODE_PARTIAL);
        assert_eq!(update_mode(true), UPDATE_MODE_FULL);
    }

    #[test]
    fn unchanged_frame_does_not_create_a_display_update_request() {
        let frame = vec![0xff; 4 * 4 * 2];
        assert_eq!(
            display_update_request(
                &frame,
                Some(&frame),
                DisplayRegion::full(4, 4),
                WaveformMode::Du,
                false,
                4,
                4,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn changed_frame_creates_the_exact_damage_request() {
        let previous = vec![0xff; 4 * 4 * 2];
        let mut frame = previous.clone();
        frame[(2 * 4 + 1) * 2..(2 * 4 + 2) * 2].copy_from_slice(&0u16.to_ne_bytes());

        assert_eq!(
            display_update_request(
                &frame,
                Some(&previous),
                DisplayRegion::new(0, 0, 4, 4),
                WaveformMode::Du,
                false,
                4,
                4,
            )
            .unwrap(),
            Some(DisplayUpdateRequest {
                region: DisplayRegion::new(1, 2, 1, 1),
                waveform: WaveformMode::Du,
                force_refresh: false,
            })
        );
    }

    #[test]
    fn forced_refresh_retains_intentional_update_request_for_identical_frame() {
        let frame = vec![0xff; 4 * 4 * 2];
        let requested_region = DisplayRegion::new(0, 0, 4, 2);

        assert_eq!(
            display_update_request(
                &frame,
                Some(&frame),
                requested_region,
                WaveformMode::Gc16,
                true,
                4,
                4,
            )
            .unwrap(),
            Some(DisplayUpdateRequest {
                region: requested_region,
                waveform: WaveformMode::Gc16,
                force_refresh: true,
            })
        );
    }

    #[test]
    fn parses_t1_waveform_modes() {
        assert_eq!(WaveformMode::parse("du").unwrap(), WaveformMode::Du);
        assert_eq!(WaveformMode::parse("GC16").unwrap(), WaveformMode::Gc16);
        assert_eq!(WaveformMode::parse("Gc4").unwrap(), WaveformMode::Gc4);
        assert_eq!(WaveformMode::parse("a2").unwrap(), WaveformMode::A2);
    }

    #[test]
    fn rejects_unknown_t1_waveform_mode() {
        assert!(WaveformMode::parse("AUTO").is_err());
    }

    #[test]
    fn render_layout_preserves_stride_padding() {
        let var = FbVarScreeninfo {
            xres: 4,
            yres: 3,
            xres_virtual: 4,
            yres_virtual: 3,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 16,
            red: FbBitfield {
                offset: 11,
                length: 5,
                ..Default::default()
            },
            green: FbBitfield {
                offset: 5,
                length: 6,
                ..Default::default()
            },
            blue: FbBitfield {
                offset: 0,
                length: 5,
                ..Default::default()
            },
            ..Default::default()
        };
        let fix = FbFixScreeninfo {
            line_length: 12,
            smem_len: 36,
            ..Default::default()
        };
        let rect = MxcfbRect {
            top: 0,
            left: 1,
            width: 2,
            height: 2,
        };
        let layout = FramebufferLayout::new(&var, &fix, rect).unwrap();
        let mut buffer = vec![0xa5; 36];
        let backup = layout.copy_visible(&buffer).unwrap();
        layout.draw_marker(&mut buffer).unwrap();
        assert_eq!(&buffer[0..2], &[0xa5, 0xa5]);
        assert_eq!(&buffer[10..12], &[0xa5, 0xa5]);
        layout.restore(&mut buffer, &backup).unwrap();
        assert_eq!(buffer, vec![0xa5; 36]);
    }

    #[test]
    fn display_canvas_honors_virtual_offsets() {
        let mut buffer = vec![0xa5; 64];
        let mut canvas = DisplayCanvas {
            buffer: &mut buffer,
            width: 2,
            height: 2,
            stride: 16,
            xoffset: 1,
            yoffset: 1,
        };
        canvas.set_pixel(0, 0, 0x1234);
        assert_eq!(&buffer[18..20], &0x1234u16.to_ne_bytes());
        assert_eq!(&buffer[0..2], &[0xa5, 0xa5]);
    }

    #[test]
    fn packed_region_copy_preserves_unrelated_pixels() {
        let mut shadow = vec![0xa5; 6 * 4 * 2];
        let pixels = [
            0x1111u16.to_ne_bytes(),
            0x2222u16.to_ne_bytes(),
            0x3333u16.to_ne_bytes(),
            0x4444u16.to_ne_bytes(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        copy_packed_region(&mut shadow, 6, DisplayRegion::new(2, 1, 2, 2), &pixels).unwrap();

        assert_eq!(&shadow[0..4], &[0xa5; 4]);
        assert_eq!(&shadow[(1 * 6 + 2) * 2..(1 * 6 + 4) * 2], &pixels[0..4]);
        assert_eq!(&shadow[(2 * 6 + 2) * 2..(2 * 6 + 4) * 2], &pixels[4..8]);
        assert_eq!(&shadow[(1 * 6 + 4) * 2..(1 * 6 + 6) * 2], &[0xa5; 4]);
    }
}
