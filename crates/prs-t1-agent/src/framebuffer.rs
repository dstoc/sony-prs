use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::raw::{c_int, c_ulong, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;

const FBIOGET_VSCREENINFO: c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: c_ulong = 0x4602;
const PROT_READ: c_int = 1;
const MAP_SHARED: c_int = 1;
const MAP_FAILED: *mut c_void = -1isize as *mut c_void;
const MAX_MAPPED_BYTES: usize = 128 * 1024 * 1024;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelInfo {
    pub offset: u32,
    pub length: u32,
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
    writeln!(output, "P5")?;
    writeln!(output, "{} {}", info.width, info.height)?;
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
    let mapped = mapping.as_slice();
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
    Ok(info)
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

impl MappedFramebuffer {
    fn new(file: &File, length: usize) -> io::Result<Self> {
        let address = unsafe {
            mmap(
                ptr::null_mut(),
                length,
                PROT_READ,
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
}

impl Drop for MappedFramebuffer {
    fn drop(&mut self) {
        let _ = unsafe { munmap(self.address, self.length) };
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
        assert_eq!(pixel_to_gray(&[0xe0, 0x07], &var), 150);
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
}
