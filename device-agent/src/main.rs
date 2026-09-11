use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::raw::c_void;
use std::os::raw::{c_int, c_ulong};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{compiler_fence, Ordering};

const FBIOGET_VSCREENINFO: c_ulong = 0x4600;
const EINKFB_TRANSFER_PANEL: c_ulong = 0x4701;
const EINKFB_UPDATE_PANEL: c_ulong = 0x4702;
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const MAP_FAILED: *mut c_void = -1isize as *mut c_void;
const MAX_EXEC_BYTES: usize = 1024 * 1024;
const MAX_SHELL_BYTES: usize = 4096;
const MAX_SHELL_OUTPUT_BYTES: usize = 64 * 1024;

#[repr(C)]
#[derive(Default)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Default)]
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

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("render") => println!("PRS1 OK RENDERED {}", render_test()),
        Some("capture") => capture_framebuffer(),
        Some("receive-exec") => receive_and_execute(),
        Some("receive-shell") => receive_and_shell(),
        _ => println!(
            "PRS1 OK PROBE fb={} input={}",
            probe_framebuffer(),
            probe_input()
        ),
    }
}

fn capture_framebuffer() {
    let result = (|| -> io::Result<()> {
        let path = Path::new("/dev/fb0");
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut info = FbVarScreeninfo::default();
        let result = unsafe { ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO, &mut info) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        if info.bits_per_pixel != 8 || info.xres == 0 || info.yres == 0 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported framebuffer format {}bpp", info.bits_per_pixel),
            ));
        }
        let stride = info.xres_virtual as usize;
        let length = stride * info.yres_virtual as usize;
        let bytes = info.xres as usize * info.yres as usize;
        let mapping = unsafe {
            mmap(
                std::ptr::null_mut(),
                length,
                PROT_READ,
                MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if mapping == MAP_FAILED {
            return Err(io::Error::last_os_error());
        }

        let mut stdout = io::stdout().lock();
        write!(
            stdout,
            "PRS1 OK FRAMEBUFFER width={} height={} format=gray8 bytes={}\n",
            info.xres, info.yres, bytes
        )?;
        let pixels = mapping.cast::<u8>();
        for y in 0..info.yres as usize {
            let row =
                unsafe { std::slice::from_raw_parts(pixels.add(y * stride), info.xres as usize) };
            stdout.write_all(row)?;
        }
        stdout.flush()?;
        let unmap_result = unsafe { munmap(mapping, length) };
        if unmap_result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })();
    if let Err(error) = result {
        println!("PRS1 ERR capture-failed-{error}");
    }
}

fn receive_and_execute() {
    let result = (|| -> io::Result<(usize, u32, u32)> {
        let mut args = std::env::args().skip(2);
        let bytes = args
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing byte count"))?
            .parse::<usize>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid byte count"))?;
        let expected_crc = u32::from_str_radix(
            &args
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing CRC32"))?,
            16,
        )
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid CRC32"))?;
        if args.next().is_some() || bytes == 0 || bytes > MAX_EXEC_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("payload must be between 1 and {MAX_EXEC_BYTES} bytes"),
            ));
        }

        let path = Path::new("/tmp/prs350-upload");
        let mut output = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)?;
        let mut input = io::stdin().lock();
        let mut remaining = bytes;
        let mut crc = 0xffff_ffffu32;
        let mut buffer = [0u8; 4096];
        while remaining != 0 {
            let requested = remaining.min(buffer.len());
            let read = input.read(&mut buffer[..requested])?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "serial upload ended before the declared length",
                ));
            }
            output.write_all(&buffer[..read])?;
            crc = crc32_update(crc, &buffer[..read]);
            remaining -= read;
        }
        output.sync_all()?;
        let crc = !crc;
        if crc != expected_crc {
            let _ = fs::remove_file(path);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("CRC32 mismatch: got {crc:08x}, expected {expected_crc:08x}"),
            ));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
        let child = Command::new(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok((bytes, crc, child.id()))
    })();

    match result {
        Ok((bytes, crc, pid)) => println!("PRS1 OK EXEC size={bytes} crc32={crc:08x} pid={pid}"),
        Err(error) => println!("PRS1 ERR exec-failed-{error}"),
    }
}

fn receive_and_shell() {
    let result = (|| -> io::Result<()> {
        let mut args = std::env::args().skip(2);
        let bytes = parse_payload_size(
            args.next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing byte count"))?,
            MAX_SHELL_BYTES,
            "shell",
        )?;
        let expected_crc = parse_crc(
            args.next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing CRC32"))?,
        )?;
        if args.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unexpected shell arguments",
            ));
        }

        let command = String::from_utf8(receive_payload(bytes, expected_crc)?).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "shell command is not UTF-8")
        })?;
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .output()?;
        let mut data = Vec::with_capacity(output.stdout.len() + output.stderr.len());
        data.extend_from_slice(&output.stdout);
        data.extend_from_slice(&output.stderr);
        let truncated = data.len() > MAX_SHELL_OUTPUT_BYTES;
        data.truncate(MAX_SHELL_OUTPUT_BYTES);
        let status = output
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |code| format!("exit={code}"));

        let mut stdout = io::stdout().lock();
        writeln!(
            stdout,
            "PRS1 OK SHELL bytes={} status={}{}",
            data.len(),
            status,
            if truncated { ",truncated=1" } else { "" }
        )?;
        stdout.write_all(&data)?;
        stdout.flush()?;
        Ok(())
    })();

    if let Err(error) = result {
        println!("PRS1 ERR shell-failed-{error}");
    }
}

fn parse_payload_size(value: String, limit: usize, kind: &str) -> io::Result<usize> {
    let bytes = value
        .parse::<usize>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid byte count"))?;
    if bytes == 0 || bytes > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{kind} payload must be between 1 and {limit} bytes"),
        ));
    }
    Ok(bytes)
}

fn parse_crc(value: String) -> io::Result<u32> {
    u32::from_str_radix(&value, 16)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid CRC32"))
}

fn receive_payload(bytes: usize, expected_crc: u32) -> io::Result<Vec<u8>> {
    let mut input = io::stdin().lock();
    let mut remaining = bytes;
    let mut crc = 0xffff_ffffu32;
    let mut payload = Vec::with_capacity(bytes);
    let mut buffer = [0u8; 4096];
    while remaining != 0 {
        let requested = remaining.min(buffer.len());
        let read = input.read(&mut buffer[..requested])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "serial payload ended before the declared length",
            ));
        }
        payload.extend_from_slice(&buffer[..read]);
        crc = crc32_update(crc, &buffer[..read]);
        remaining -= read;
    }
    let crc = !crc;
    if crc != expected_crc {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("CRC32 mismatch: got {crc:08x}, expected {expected_crc:08x}"),
        ));
    }
    Ok(payload)
}

fn crc32_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    crc
}

fn probe_framebuffer() -> String {
    let path = Path::new("/dev/fb0");
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) => {
            return format!("unavailable-{error}");
        }
    };

    let mut info = FbVarScreeninfo::default();
    let result = unsafe { ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO, &mut info) };
    if result != 0 {
        return format!("ioctl-failed-{}", io::Error::last_os_error());
    }
    format!(
        "{}x{}-virtual={}x{}-bpp={}-gray={}-rotate={}-rgb={}@{},{}@{},{}@{}",
        info.xres,
        info.yres,
        info.xres_virtual,
        info.yres_virtual,
        info.bits_per_pixel,
        info.grayscale,
        info.rotate,
        info.red.length,
        info.red.offset,
        info.green.length,
        info.green.offset,
        info.blue.length,
        info.blue.offset
    )
}

fn probe_input() -> String {
    let mut devices = Vec::new();
    let directory = match fs::read_dir("/dev/input") {
        Ok(directory) => directory,
        Err(error) => {
            return format!("unavailable-{error}");
        }
    };

    for entry in directory.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("event"))
        {
            devices.push(path.display().to_string());
        }
    }
    devices.sort();
    if devices.is_empty() {
        "none".into()
    } else {
        devices.join(",")
    }
}

fn render_test() -> String {
    let path = Path::new("/dev/fb0");
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) => return format!("unavailable-{error}"),
    };

    let mut info = FbVarScreeninfo::default();
    let result = unsafe { ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO, &mut info) };
    if result != 0 {
        return format!("ioctl-failed-{}", io::Error::last_os_error());
    }
    if info.bits_per_pixel != 8 || info.xres_virtual == 0 || info.yres_virtual == 0 {
        return format!(
            "unsupported-format-{}x{}-bpp={}",
            info.xres_virtual, info.yres_virtual, info.bits_per_pixel
        );
    }

    let stride = info.xres_virtual as usize;
    let length = stride * info.yres_virtual as usize;
    let mapping = unsafe {
        mmap(
            std::ptr::null_mut(),
            length,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if mapping == MAP_FAILED {
        return format!("mmap-failed-{}", io::Error::last_os_error());
    }

    let pixels = mapping.cast::<u8>();
    for y in 0..info.yres as usize {
        for x in 0..info.xres as usize {
            let tile = ((x / 32) + (y / 32)) % 2;
            let value = if tile == 0 { 0x00 } else { 0xff };
            unsafe { pixels.add(y * stride + x).write_volatile(value) };
        }
    }

    let mut transfer = [0u32, 0, 0, 0, info.xres, info.yres];
    let transfer_result = unsafe {
        ioctl(
            file.as_raw_fd(),
            EINKFB_TRANSFER_PANEL,
            transfer.as_mut_ptr(),
        )
    };
    let transfer_error = io::Error::last_os_error();
    if transfer_result != 0 {
        unsafe { munmap(mapping, length) };
        return format!("transfer-failed-{}", transfer_error);
    }

    let mut update = [1u32, 0, 0, 0, info.xres, info.yres];
    let update_result =
        unsafe { ioctl(file.as_raw_fd(), EINKFB_UPDATE_PANEL, update.as_mut_ptr()) };
    let update_error = io::Error::last_os_error();
    let unmap_result = unsafe { munmap(mapping, length) };
    if update_result != 0 {
        return format!("refresh-failed-{}", update_error);
    }
    if unmap_result != 0 {
        return format!("munmap-failed-{}", io::Error::last_os_error());
    }
    format!("{}x{}-checkerboard", info.xres, info.yres)
}

// The reader's ARMv6 userspace has no matching static libgcc. These helpers
// are sufficient for this single-threaded bootstrap probe; a production agent
// must replace them with proper ARM atomic support before spawning threads.
#[no_mangle]
pub unsafe extern "C" fn __sync_val_compare_and_swap_4(
    pointer: *mut i32,
    old: i32,
    new: i32,
) -> i32 {
    let current = unsafe { pointer.read_volatile() };
    if current == old {
        unsafe { pointer.write_volatile(new) };
    }
    current
}

#[no_mangle]
pub unsafe extern "C" fn __sync_fetch_and_add_4(pointer: *mut i32, value: i32) -> i32 {
    let current = unsafe { pointer.read_volatile() };
    unsafe { pointer.write_volatile(current.wrapping_add(value)) };
    current
}

#[no_mangle]
pub unsafe extern "C" fn __sync_fetch_and_sub_4(pointer: *mut i32, value: i32) -> i32 {
    let current = unsafe { pointer.read_volatile() };
    unsafe { pointer.write_volatile(current.wrapping_sub(value)) };
    current
}

#[no_mangle]
pub unsafe extern "C" fn __sync_fetch_and_or_4(pointer: *mut i32, value: i32) -> i32 {
    let current = unsafe { pointer.read_volatile() };
    unsafe { pointer.write_volatile(current | value) };
    current
}

#[no_mangle]
pub unsafe extern "C" fn __sync_lock_test_and_set_4(pointer: *mut i32, value: i32) -> i32 {
    let current = unsafe { pointer.read_volatile() };
    unsafe { pointer.write_volatile(value) };
    current
}

#[no_mangle]
pub unsafe extern "C" fn __sync_val_compare_and_swap_1(pointer: *mut i8, old: i8, new: i8) -> i8 {
    let current = unsafe { pointer.read_volatile() };
    if current == old {
        unsafe { pointer.write_volatile(new) };
    }
    current
}

#[no_mangle]
pub unsafe extern "C" fn __sync_lock_test_and_set_1(pointer: *mut i8, value: i8) -> i8 {
    let current = unsafe { pointer.read_volatile() };
    unsafe { pointer.write_volatile(value) };
    current
}

#[no_mangle]
pub extern "C" fn __sync_synchronize() {
    compiler_fence(Ordering::SeqCst);
}
