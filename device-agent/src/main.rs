use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::raw::c_void;
use std::os::raw::{c_int, c_ulong};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{compiler_fence, Ordering};
use std::time::{Duration, Instant};

const FBIOGET_VSCREENINFO: c_ulong = 0x4600;
const EINKFB_UPDATE_PIC: c_ulong = 0x4700;
const EINKFB_SET_POWER_MODE: c_ulong = 0x46f6;
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const MAP_FAILED: *mut c_void = -1isize as *mut c_void;
const MAX_EXEC_BYTES: usize = 1024 * 1024;
const MAX_SHELL_BYTES: usize = 4096;
const MAX_SHELL_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_PROTOCOL_LINE: usize = 1024;
const TRANSFER_CHUNK_SIZE: usize = 1024;
const O_NONBLOCK: i32 = 0x800;
const POLLIN: i16 = 0x0001;
const POLLERR: i16 = 0x0008;
const POLLHUP: i16 = 0x0010;
const POLLNVAL: i16 = 0x0020;
const SUBCPU_PACKET_SIZE: usize = 8;
const SUBCPU_SCAN_ON_PACKET: [u8; SUBCPU_PACKET_SIZE] =
    pack_subcpu_packet([0x86, 0x03, 0x31, 0x00, 0x00, 0x00, 0xb4]);

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
    fn poll(fds: *mut PollFd, nfds: usize, timeout: c_int) -> c_int;
}

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

#[repr(C)]
struct EinkUpdate {
    upmode: u32,
    orientation: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Clone, Copy, Debug)]
enum InputEvent {
    Touch { pressed: bool, x: u16, y: u16 },
    Key { code: u8, state: u8 },
}

#[derive(Debug, PartialEq, Eq)]
enum ServiceRequest {
    Ping,
    Info,
    Status,
    Reboot,
    Probe,
    Render,
    Capture,
    Execute { bytes: usize, crc32: u32 },
    Shell { bytes: usize, crc32: u32 },
}

#[derive(Default)]
struct UiState {
    touch: Option<(u16, u16, bool)>,
    key: Option<(u8, u8)>,
}

struct SubCpuInput {
    file: std::fs::File,
    pending: Vec<u8>,
}

impl SubCpuInput {
    fn open() -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open("/dev/subcpu")?;
        file.write_all(&SUBCPU_SCAN_ON_PACKET)?;
        Ok(Self {
            file,
            pending: Vec::with_capacity(SUBCPU_PACKET_SIZE * 2),
        })
    }

    fn next_event(&mut self) -> io::Result<Option<InputEvent>> {
        let mut descriptor = PollFd {
            fd: self.file.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        };
        let result = unsafe { poll(&mut descriptor, 1, 100) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(None);
            }
            return Err(error);
        }
        if result == 0 {
            return Ok(None);
        }
        if descriptor.revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "subcpu input stream closed",
            ));
        }
        if descriptor.revents & POLLIN == 0 {
            return Ok(None);
        }

        let mut bytes = [0u8; 64];
        match self.file.read(&mut bytes) {
            Ok(0) => return Ok(None),
            Ok(length) => self.pending.extend_from_slice(&bytes[..length]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(error) => return Err(error),
        }

        while self.pending.len() >= SUBCPU_PACKET_SIZE {
            if self.pending[0] & 0x80 == 0 {
                self.pending.remove(0);
                continue;
            }
            let packet: [u8; SUBCPU_PACKET_SIZE] = self.pending[..SUBCPU_PACKET_SIZE]
                .try_into()
                .expect("packet length checked");
            self.pending.drain(..SUBCPU_PACKET_SIZE);
            if let Some(event) = decode_subcpu_packet(packet) {
                return Ok(Some(event));
            }
        }
        Ok(None)
    }
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("render") => println!("PRS1 OK RENDERED {}", render_test()),
        Some("capture") => capture_framebuffer(),
        Some("ui") => run_ui(),
        Some("service") => run_serial_service(),
        Some("receive-exec") => receive_and_execute(),
        Some("receive-shell") => receive_and_shell(),
        Some("watch-usb") => watch_usb(std::env::args().nth(2)),
        _ => println!(
            "PRS1 OK PROBE fb={} input={}",
            probe_framebuffer(),
            probe_input()
        ),
    }
}

fn run_serial_service() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let result = run_serial_service_with(stdin.lock(), stdout.lock());
    if let Err(error) = result {
        eprintln!("Rust serial service stopped: {error}");
    }
}

fn run_serial_service_with(mut input: impl Read, mut output: impl Write) -> io::Result<()> {
    loop {
        let Some(line) = read_protocol_line(&mut input)? else {
            return Ok(());
        };
        let request = match parse_service_request(&line) {
            Ok(request) => request,
            Err(error) => {
                write_protocol_error(&mut output, &error)?;
                continue;
            }
        };

        let result = handle_service_request(request, &mut input, &mut output);
        if let Err(error) = result {
            write_protocol_error(&mut output, &error)?;
        }
    }
}

fn handle_service_request(
    request: ServiceRequest,
    input: &mut impl Read,
    output: &mut impl Write,
) -> io::Result<()> {
    match request {
        ServiceRequest::Ping => write_protocol_line(output, "PRS1 OK PONG"),
        ServiceRequest::Info => {
            write_protocol_line(output, "PRS1 OK INFO model=PRS-350 transport=cdc-acm")
        }
        ServiceRequest::Status => {
            let ui = if stock_ui_alive() {
                "stock-alive"
            } else {
                "stock-missing"
            };
            write_protocol_line(output, &format!("PRS1 OK STATUS ui={ui}"))
        }
        ServiceRequest::Reboot => {
            write_protocol_line(output, "PRS1 OK REBOOTING")?;
            let _ = Command::new("/bin/sync").status();
            std::thread::sleep(Duration::from_secs(1));
            let _ = Command::new("/sbin/reboot").status();
            Ok(())
        }
        ServiceRequest::Probe => write_protocol_line(
            output,
            &format!(
                "PRS1 OK PROBE fb={} input={}",
                probe_framebuffer(),
                probe_input()
            ),
        ),
        ServiceRequest::Render => {
            write_protocol_line(output, &format!("PRS1 OK RENDERED {}", render_test()))
        }
        ServiceRequest::Capture => capture_framebuffer_to(output),
        ServiceRequest::Execute { bytes, crc32 } => {
            set_serial_mode(true)?;
            write_protocol_line(output, "PRS1 READY EXEC")?;
            let payload_result = receive_payload_from(input, output, bytes, crc32, "EXEC");
            set_serial_mode(false)?;
            let payload = payload_result?;
            let (size, crc, pid) = execute_binary(&payload)?;
            write_protocol_line(
                output,
                &format!("PRS1 OK EXEC size={size} crc32={crc:08x} pid={pid}"),
            )
        }
        ServiceRequest::Shell { bytes, crc32 } => {
            set_serial_mode(true)?;
            write_protocol_line(output, "PRS1 READY SHELL")?;
            let payload_result = receive_payload_from(input, output, bytes, crc32, "SHELL");
            set_serial_mode(false)?;
            let command = String::from_utf8(payload_result?)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "shell command is not UTF-8"))?;
            let result = Command::new("/bin/sh")
                .arg("-c")
                .arg(command)
                .stdin(Stdio::null())
                .output()?;
            let mut data = Vec::with_capacity(result.stdout.len() + result.stderr.len());
            data.extend_from_slice(&result.stdout);
            data.extend_from_slice(&result.stderr);
            let truncated = data.len() > MAX_SHELL_OUTPUT_BYTES;
            data.truncate(MAX_SHELL_OUTPUT_BYTES);
            let status = result
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |code| format!("exit={code}"));
            write_protocol_line(
                output,
                &format!(
                    "PRS1 OK SHELL bytes={} status={}{}",
                    data.len(),
                    status,
                    if truncated { ",truncated=1" } else { "" }
                ),
            )?;
            output.write_all(&data)?;
            output.flush()
        }
    }
}

fn set_serial_mode(raw: bool) -> io::Result<()> {
    let args = if raw {
        [
            "9600", "raw", "-echo", "-ixon", "-ixoff", "min", "0", "time", "10",
        ]
    } else {
        [
            "9600", "icanon", "-echo", "-ixon", "-ixoff", "min", "1", "time", "0",
        ]
    };
    let status = Command::new("/bin/stty").args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            if raw {
                "could not enter serial upload mode"
            } else {
                "could not restore serial line mode"
            },
        ))
    }
}

fn parse_service_request(line: &[u8]) -> io::Result<ServiceRequest> {
    let line = String::from_utf8(line.to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request is not UTF-8"))?;
    let mut fields = line.trim_end_matches(['\r', '\n']).split_whitespace();
    if fields.next() != Some("PRS1") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported serial request",
        ));
    }
    let command = fields.next().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "serial request is missing a command")
    })?;
    let request = match command {
        "PING" => ServiceRequest::Ping,
        "INFO" => ServiceRequest::Info,
        "STATUS" => ServiceRequest::Status,
        "REBOOT" => ServiceRequest::Reboot,
        "PROBE" => ServiceRequest::Probe,
        "RENDER" => ServiceRequest::Render,
        "CAPTURE" => ServiceRequest::Capture,
        "EXEC" | "SHELL" => {
            let bytes = fields
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing byte count"))?
                .parse::<usize>()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid byte count"))?;
            let limit = if command == "EXEC" {
                MAX_EXEC_BYTES
            } else {
                MAX_SHELL_BYTES
            };
            if bytes == 0 || bytes > limit {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("payload must be between 1 and {limit} bytes"),
                ));
            }
            let crc32 = u32::from_str_radix(
                fields.next().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "missing CRC32")
                })?,
                16,
            )
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid CRC32"))?;
            if fields.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unexpected serial request arguments",
                ));
            }
            if command == "EXEC" {
                ServiceRequest::Execute { bytes, crc32 }
            } else {
                ServiceRequest::Shell { bytes, crc32 }
            }
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported serial request",
            ))
        }
    };
    if fields.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unexpected serial request arguments",
        ));
    }
    Ok(request)
}

fn read_protocol_line(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match input.read(&mut byte)? {
            0 => return Ok(if line.is_empty() { None } else { Some(line) }),
            1 => {
                line.push(byte[0]);
                if line.len() > MAX_PROTOCOL_LINE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "serial request line is too long",
                    ));
                }
                if byte[0] == b'\n' {
                    return Ok(Some(line));
                }
            }
            _ => unreachable!(),
        }
    }
}

fn write_protocol_line(output: &mut impl Write, line: &str) -> io::Result<()> {
    if line.len() + 1 > MAX_PROTOCOL_LINE || line.bytes().any(|byte| byte == b'\r' || byte == b'\n')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "serial response line is invalid",
        ));
    }
    output.write_all(line.as_bytes())?;
    output.write_all(b"\n")?;
    output.flush()
}

fn write_protocol_error(output: &mut impl Write, error: &io::Error) -> io::Result<()> {
    let message = error.to_string().replace(['\r', '\n'], " ");
    write_protocol_line(output, &format!("PRS1 ERR {message}"))
}

fn stock_ui_alive() -> bool {
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
                .map(|_| entry.path().join("cmdline"))
        })
        .filter_map(|path| fs::read(path).ok())
        .any(|cmdline| cmdline.windows(b"tinyhttp".len()).any(|part| part == b"tinyhttp"))
}

fn run_ui() {
    let mut state = UiState::default();
    if let Err(error) = draw_ui(&state) {
        eprintln!("Rust UI failed: {error}");
        return;
    }

    let mut input = None;
    let mut retry_input_at = Instant::now();
    loop {
        if input.is_none() && retry_input_at.elapsed() >= Duration::from_secs(1) {
            match SubCpuInput::open() {
                Ok(device) => input = Some(device),
                Err(error) => eprintln!("Rust UI input unavailable: {error}"),
            }
            retry_input_at = Instant::now();
        }

        let Some(device) = input.as_mut() else {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        match device.next_event() {
            Ok(Some(event)) => {
                let redraw = match event {
                    InputEvent::Touch { pressed, x, y } => {
                        state.touch = Some((x, y, pressed));
                        !pressed
                    }
                    InputEvent::Key { code, state: key_state } => {
                        state.key = Some((code, key_state));
                        true
                    }
                };
                if redraw {
                    if let Err(error) = draw_ui(&state) {
                        eprintln!("Rust UI input redraw failed: {error}");
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("Rust UI input stopped: {error}");
                input = None;
                retry_input_at = Instant::now();
            }
        }
    }
}

fn draw_ui(state: &UiState) -> io::Result<()> {
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

    let width = info.xres as usize;
    let height = info.yres as usize;
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
        return Err(io::Error::last_os_error());
    }

    let pixels = unsafe { std::slice::from_raw_parts_mut(mapping.cast::<u8>(), length) };
    pixels.fill(0xff);
    fill_rect(pixels, stride, width, height, 0, 0, width, 88, 0x00);
    draw_text(pixels, stride, width, height, 32, 28, "PRS-350", 4, 0xff);
    draw_text(pixels, stride, width, height, 32, 124, "RUST UI", 5, 0x00);

    stroke_rect(pixels, stride, width, height, 28, 210, width - 56, 136, 0x00);
    draw_text(pixels, stride, width, height, 52, 242, "SIGNED PACKAGE", 3, 0x00);
    draw_text(pixels, stride, width, height, 52, 290, "STATUS: READY", 3, 0x00);

    stroke_rect(pixels, stride, width, height, 28, 382, width - 56, 136, 0x00);
    draw_text(pixels, stride, width, height, 52, 414, "FRAMEBUFFER", 3, 0x00);
    draw_text(pixels, stride, width, height, 52, 462, "600 X 800 GRAY8", 3, 0x00);

    draw_text(pixels, stride, width, height, 32, 558, "INPUT READY", 3, 0x00);
    if let Some((x, y, pressed)) = state.touch {
        let touch = format!("TOUCH {} {} {}", x, y, if pressed { 1 } else { 0 });
        draw_text(pixels, stride, width, height, 32, 606, &touch, 3, 0x00);
    } else {
        draw_text(pixels, stride, width, height, 32, 606, "TOUCH WAIT", 3, 0x00);
    }
    if let Some((code, key_state)) = state.key {
        let key = format!("KEY {} {}", code, key_state);
        draw_text(pixels, stride, width, height, 32, 654, &key, 3, 0x00);
    } else {
        draw_text(pixels, stride, width, height, 32, 654, "KEY WAIT", 3, 0x00);
    }
    draw_text(pixels, stride, width, height, 32, height.saturating_sub(64), "USB DIAGNOSTICS", 3, 0x00);

    let power_result = unsafe { ioctl(file.as_raw_fd(), EINKFB_SET_POWER_MODE, 1u32) };
    if power_result != 0 {
        let error = io::Error::last_os_error();
        unsafe { munmap(mapping, length) };
        return Err(io::Error::new(
            error.kind(),
            format!("set E-Ink power mode: {error}"),
        ));
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mut update = EinkUpdate {
        upmode: 3,
        orientation: 1,
        x: 0,
        y: 0,
        w: info.xres,
        h: info.yres,
    };
    let update_result = unsafe {
        ioctl(
            file.as_raw_fd(),
            EINKFB_UPDATE_PIC,
            &mut update,
        )
    };
    let update_error = io::Error::last_os_error();
    let unmap_result = unsafe { munmap(mapping, length) };
    if update_result != 0 {
        return Err(io::Error::new(
            update_error.kind(),
            format!("update E-Ink picture: {update_error}"),
        ));
    }
    if unmap_result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

const fn pack_subcpu_packet(raw: [u8; 7]) -> [u8; SUBCPU_PACKET_SIZE] {
    [
        0x80 | (raw[0] >> 1),
        ((raw[0] << 6) | (raw[1] >> 2)) & 0x7f,
        ((raw[1] << 5) | (raw[2] >> 3)) & 0x7f,
        ((raw[2] << 4) | (raw[3] >> 4)) & 0x7f,
        ((raw[3] << 3) | (raw[4] >> 5)) & 0x7f,
        ((raw[4] << 2) | (raw[5] >> 6)) & 0x7f,
        ((raw[5] << 1) | (raw[6] >> 7)) & 0x7f,
        raw[6] & 0x7f,
    ]
}

fn unpack_subcpu_packet(packet: [u8; SUBCPU_PACKET_SIZE]) -> [u8; 7] {
    [
        (packet[0] << 1) | (packet[1] >> 6),
        (packet[1] << 2) | (packet[2] >> 5),
        (packet[2] << 3) | (packet[3] >> 4),
        (packet[3] << 4) | (packet[4] >> 3),
        (packet[4] << 5) | (packet[5] >> 2),
        (packet[5] << 6) | (packet[6] >> 1),
        (packet[6] << 7) | packet[7],
    ]
}

fn decode_subcpu_packet(packet: [u8; SUBCPU_PACKET_SIZE]) -> Option<InputEvent> {
    let raw = unpack_subcpu_packet(packet);
    if raw.iter().fold(0u8, |checksum, byte| checksum ^ byte) != 0 {
        return None;
    }

    let category = raw[0] & 0x3f;
    let command = raw[1] & 0x3f;
    match (category, command) {
        (3, 1) => Some(InputEvent::Key {
            code: raw[2],
            state: raw[3],
        }),
        (6, 4) | (6, 5) | (6, 6) => {
            let x = (((raw[2] & 0x0f) as u16) << 8) | raw[3] as u16;
            let y = (((raw[4] & 0x0f) as u16) << 8) | raw[5] as u16;
            let x = interpolate_touch(x, 513, 3588, 100, 500, 599);
            let y = interpolate_touch(y, 3411, 677, 100, 700, 799);
            Some(InputEvent::Touch {
                pressed: command != 5,
                x,
                y,
            })
        }
        _ => None,
    }
}

fn interpolate_touch(
    value: u16,
    raw_start: i32,
    raw_end: i32,
    screen_start: i32,
    screen_end: i32,
    screen_max: i32,
) -> u16 {
    let value = i32::from(value);
    let numerator = (value - raw_start) * (screen_end - screen_start);
    let denominator = raw_end - raw_start;
    let mapped = screen_start + numerator / denominator;
    mapped.clamp(0, screen_max) as u16
}

fn fill_rect(pixels: &mut [u8], stride: usize, width: usize, height: usize, x: usize, y: usize, rect_width: usize, rect_height: usize, value: u8) {
    let right = x.saturating_add(rect_width).min(width);
    let bottom = y.saturating_add(rect_height).min(height);
    for row in y.min(height)..bottom {
        let offset = row * stride + x.min(width);
        pixels[offset..row * stride + right].fill(value);
    }
}

fn stroke_rect(pixels: &mut [u8], stride: usize, width: usize, height: usize, x: usize, y: usize, rect_width: usize, rect_height: usize, value: u8) {
    fill_rect(pixels, stride, width, height, x, y, rect_width, 3, value);
    fill_rect(pixels, stride, width, height, x, y.saturating_add(rect_height).saturating_sub(3), rect_width, 3, value);
    fill_rect(pixels, stride, width, height, x, y, 3, rect_height, value);
    fill_rect(pixels, stride, width, height, x.saturating_add(rect_width).saturating_sub(3), y, 3, rect_height, value);
}

fn draw_text(pixels: &mut [u8], stride: usize, width: usize, height: usize, x: usize, y: usize, text: &str, scale: usize, value: u8) {
    let mut cursor = x;
    for byte in text.bytes() {
        if byte == b' ' {
            cursor = cursor.saturating_add(6 * scale);
            continue;
        }
        let glyph = glyph(byte);
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) != 0 {
                    fill_rect(pixels, stride, width, height, cursor + column * scale, y + row * scale, scale, scale, value);
                }
            }
        }
        cursor = cursor.saturating_add(6 * scale);
    }
}

fn glyph(byte: u8) -> [u8; 7] {
    match byte {
        b'0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        b'1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        b'3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        b'4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        b'5' => [0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110],
        b'6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        b'7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        b'8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        b'9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b11100],
        b'-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        b':' => [0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000],
        b'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        b'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        b'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        b'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        b'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        b'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        b'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
        b'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        b'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        b'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        b'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        b'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        b'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        b'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        b'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        b'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        b'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        b'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        b'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        b'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        b'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        b'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        b'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        _ => [0; 7],
    }
}

fn watch_usb(path: Option<String>) {
    let Some(path) = path else {
        return;
    };
    let result = (|| -> io::Result<()> {
        let mut armed = false;
        loop {
            let file = match OpenOptions::new()
                .read(true)
                .custom_flags(O_NONBLOCK)
                .open(&path)
            {
                Ok(file) => file,
                Err(_) => {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    continue;
                }
            };
            let mut descriptor = PollFd {
                fd: file.as_raw_fd(),
                events: 0,
                revents: 0,
            };
            let result = unsafe { poll(&mut descriptor, 1, 1000) };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if descriptor.revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
                if armed {
                    let _ = Command::new("/bin/sync").status();
                    let _ = Command::new("/sbin/reboot").status();
                    return Ok(());
                }
                continue;
            }
            armed = true;
        }
    })();
    if let Err(error) = result {
        eprintln!("USB recovery watcher stopped: {error}");
    }
}

fn capture_framebuffer() {
    let mut stdout = io::stdout().lock();
    let result = capture_framebuffer_to(&mut stdout);
    if let Err(error) = result {
        let _ = write_protocol_error(&mut stdout, &io::Error::new(
            error.kind(),
            format!("capture-failed-{error}"),
        ));
    }
}

fn capture_framebuffer_to(output: &mut impl Write) -> io::Result<()> {
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

    write_protocol_line(
        output,
        &format!(
            "PRS1 OK FRAMEBUFFER width={} height={} format=gray8 bytes={}",
            info.xres, info.yres, bytes
        ),
    )?;
    let pixels = mapping.cast::<u8>();
    for y in 0..info.yres as usize {
        let row =
            unsafe { std::slice::from_raw_parts(pixels.add(y * stride), info.xres as usize) };
        output.write_all(row)?;
    }
    output.flush()?;
    let unmap_result = unsafe { munmap(mapping, length) };
    if unmap_result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
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
        let mut stdout = io::stdout().lock();
        let mut remaining = bytes;
        let mut received = 0usize;
        let mut crc = 0xffff_ffffu32;
        let mut buffer = [0u8; TRANSFER_CHUNK_SIZE];
        while remaining != 0 {
            let requested = remaining.min(buffer.len());
            input.read_exact(&mut buffer[..requested])?;
            output.write_all(&buffer[..requested])?;
            crc = crc32_update(crc, &buffer[..requested]);
            remaining -= requested;
            received += requested;
            write_ack(&mut stdout, "EXEC", received)?;
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
        drop(output);
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

        let command = String::from_utf8(receive_payload(bytes, expected_crc, "SHELL")?).map_err(|_| {
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

fn receive_payload(bytes: usize, expected_crc: u32, kind: &str) -> io::Result<Vec<u8>> {
    let mut input = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    receive_payload_from(&mut input, &mut stdout, bytes, expected_crc, kind)
}

fn receive_payload_from(
    input: &mut impl Read,
    stdout: &mut impl Write,
    bytes: usize,
    expected_crc: u32,
    kind: &str,
) -> io::Result<Vec<u8>> {
    let mut remaining = bytes;
    let mut received = 0usize;
    let mut crc = 0xffff_ffffu32;
    let mut payload = Vec::with_capacity(bytes);
    let mut buffer = [0u8; TRANSFER_CHUNK_SIZE];
    while remaining != 0 {
        let requested = remaining.min(buffer.len());
        input.read_exact(&mut buffer[..requested])?;
        payload.extend_from_slice(&buffer[..requested]);
        crc = crc32_update(crc, &buffer[..requested]);
        remaining -= requested;
        received += requested;
        write_ack(stdout, kind, received)?;
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

fn execute_binary(payload: &[u8]) -> io::Result<(usize, u32, u32)> {
    let path = Path::new("/tmp/prs350-upload");
    let mut output = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    output.write_all(payload)?;
    output.sync_all()?;
    drop(output);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    let child = Command::new(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let crc = !crc32_update(0xffff_ffff, payload);
    Ok((payload.len(), crc, child.id()))
}

fn write_ack(stdout: &mut impl Write, kind: &str, bytes: usize) -> io::Result<()> {
    writeln!(stdout, "PRS1 ACK {kind} bytes={bytes}")?;
    stdout.flush()
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

    let power_result = unsafe { ioctl(file.as_raw_fd(), EINKFB_SET_POWER_MODE, 1u32) };
    if power_result != 0 {
        let error = io::Error::last_os_error();
        unsafe { munmap(mapping, length) };
        return format!("power-mode-failed-{error}");
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mut update = EinkUpdate {
        upmode: 3,
        orientation: 1,
        x: 0,
        y: 0,
        w: info.xres,
        h: info.yres,
    };
    let update_result = unsafe { ioctl(file.as_raw_fd(), EINKFB_UPDATE_PIC, &mut update) };
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{
        decode_subcpu_packet, interpolate_touch, pack_subcpu_packet, parse_service_request,
        receive_payload_from, InputEvent, ServiceRequest, SUBCPU_PACKET_SIZE,
        crc32_update,
    };

    #[test]
    fn scan_packet_matches_firmware_encoding() {
        assert_eq!(
            pack_subcpu_packet([0x86, 0x03, 0x31, 0x00, 0x00, 0x00, 0xb4]),
            [0xc3, 0x00, 0x66, 0x10, 0x00, 0x00, 0x01, 0x34]
        );
    }

    #[test]
    fn decodes_touch_release_and_calibrates_coordinates() {
        let packet = [0x83, 0x01, 0x29, 0x01, 0x40, 0x20, 0x1e, 0x54];
        assert!(matches!(
            decode_subcpu_packet(packet),
            Some(InputEvent::Touch {
                pressed: false,
                x: 302,
                y: 395
            })
        ));
    }

    #[test]
    fn decodes_key_packet() {
        let packet = [0x81, 0x40, 0x25, 0x30, 0x10, 0x00, 0x00, 0x2b];
        assert!(matches!(
            decode_subcpu_packet(packet),
            Some(InputEvent::Key {
                code: 0x2b,
                state: 0x02
            })
        ));
        assert_eq!(SUBCPU_PACKET_SIZE, 8);
    }

    #[test]
    fn ignores_bad_checksum() {
        let packet = [0x83, 0x01, 0x29, 0x01, 0x40, 0x20, 0x1e, 0x55];
        assert_eq!(decode_subcpu_packet(packet), None);
    }

    #[test]
    fn calibration_preserves_panel_dimensions() {
        assert_eq!(interpolate_touch(513, 513, 3588, 100, 500, 599), 100);
        assert_eq!(interpolate_touch(3588, 513, 3588, 100, 500, 599), 500);
        assert_eq!(interpolate_touch(3411, 3411, 677, 100, 700, 799), 100);
        assert_eq!(interpolate_touch(677, 3411, 677, 100, 700, 799), 700);
    }

    #[test]
    fn parses_service_requests() {
        assert_eq!(
            parse_service_request(b"PRS1 PING\n").unwrap(),
            ServiceRequest::Ping
        );
        assert_eq!(
            parse_service_request(b"PRS1 EXEC 12 deadbeef\r\n").unwrap(),
            ServiceRequest::Execute {
                bytes: 12,
                crc32: 0xdead_beef
            }
        );
        assert!(parse_service_request(b"PRS1 SHELL 0 00000000\n").is_err());
        assert!(parse_service_request(b"PRS1 UNKNOWN\n").is_err());
    }

    #[test]
    fn receives_payload_and_acknowledges_each_chunk() {
        let payload = b"test payload";
        let crc = !crc32_update(0xffff_ffff, payload);
        let mut input = Cursor::new(payload.to_vec());
        let mut output = Vec::new();
        let received = receive_payload_from(
            &mut input,
            &mut output,
            payload.len(),
            crc,
            "SHELL",
        )
        .unwrap();
        assert_eq!(received, payload);
        assert_eq!(output, b"PRS1 ACK SHELL bytes=12\n");
    }
}
