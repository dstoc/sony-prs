mod framebuffer;
mod input;
mod service;
mod service_protocol;
mod transfer;
mod usb;

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

use prs350_wire::{
    MAX_EXEC_BYTES, MAX_LINE_LEN as MAX_PROTOCOL_LINE, MAX_SHELL_BYTES, MAX_SHELL_OUTPUT_BYTES,
};

const FBIOGET_VSCREENINFO: c_ulong = 0x4600;
const EINKFB_UPDATE_PIC: c_ulong = 0x4700;
const EINKFB_SET_POWER_MODE: c_ulong = 0x46f6;
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const MAP_FAILED: *mut c_void = -1isize as *mut c_void;
const TRANSFER_CHUNK_SIZE: usize = 1024;
const O_NONBLOCK: i32 = 0x800;
const POLLIN: i16 = 0x0001;
const POLLERR: i16 = 0x0008;
const POLLHUP: i16 = 0x0010;
const POLLNVAL: i16 = 0x0020;
const SUBCPU_PACKET_SIZE: usize = 8;
const SUBCPU_SCAN_ON_PACKET: [u8; SUBCPU_PACKET_SIZE] =
    input::pack_subcpu_packet([0x86, 0x03, 0x31, 0x00, 0x00, 0x00, 0xb4]);

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
#[derive(Default)]
struct UiState {
    touch: Option<(u16, u16, bool)>,
    key: Option<(u8, u8)>,
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("render") => println!("PRS1 OK RENDERED {}", framebuffer::render_test()),
        Some("capture") => service::capture_framebuffer(),
        Some("ui") => run_ui(),
        Some("service") => service::run_serial_service(),
        Some("receive-exec") => transfer::receive_and_execute(),
        Some("receive-shell") => transfer::receive_and_shell(),
        Some("watch-usb") => usb::watch_usb(std::env::args().nth(2)),
        _ => println!(
            "PRS1 OK PROBE fb={} input={}",
            framebuffer::probe_framebuffer(),
            framebuffer::probe_input()
        ),
    }
}

fn run_ui() {
    let mut state = UiState::default();
    if let Err(error) = framebuffer::draw_ui(&state) {
        eprintln!("Rust UI failed: {error}");
        return;
    }

    let mut input = None;
    let mut retry_input_at = Instant::now();
    loop {
        if input.is_none() && retry_input_at.elapsed() >= Duration::from_secs(1) {
            match input::SubCpuInput::open() {
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
                    input::InputEvent::Touch { pressed, x, y } => {
                        state.touch = Some((x, y, pressed));
                        !pressed
                    }
                    input::InputEvent::Key {
                        code,
                        state: key_state,
                    } => {
                        state.key = Some((code, key_state));
                        true
                    }
                };
                if redraw {
                    if let Err(error) = framebuffer::draw_ui(&state) {
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
        input::{decode_subcpu_packet, interpolate_touch, pack_subcpu_packet, InputEvent},
        service_protocol::{parse_service_request, ServiceRequest},
        transfer::{crc32_update, receive_payload_from},
        SUBCPU_PACKET_SIZE,
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
        let received =
            receive_payload_from(&mut input, &mut output, payload.len(), crc, "SHELL").unwrap();
        assert_eq!(received, payload);
        assert_eq!(output, b"PRS1 ACK SHELL bytes=12\n");
    }
}
