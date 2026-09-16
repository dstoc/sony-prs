mod android;
mod damage;
mod display;
mod framebuffer;
mod input;
mod runtime;
mod status;

use crate::framebuffer::WaveformMode;
use std::env;
use std::io;
use std::path::Path;
use std::sync::atomic::{compiler_fence, Ordering};
use std::time::Duration;

const DEFAULT_FRAMEBUFFER: &str = "/dev/graphics/fb0";

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(2);
    }
}

fn run() -> io::Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("help") | Some("--help") | Some("-h") => {
            print_usage();
            Ok(())
        }
        Some("probe") => {
            let device = args.next().unwrap_or_else(|| DEFAULT_FRAMEBUFFER.into());
            if args.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "probe accepts at most one framebuffer path",
                ));
            }
            probe(Path::new(&device));
            Ok(())
        }
        Some("input") => {
            if args.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "input accepts no arguments",
                ));
            }
            input::print_inventory();
            Ok(())
        }
        Some("status") => {
            if args.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "status accepts no arguments",
                ));
            }
            status::print_status();
            Ok(())
        }
        Some("events") => {
            let device = args.next().unwrap_or_else(|| "/dev/input/event1".into());
            let seconds = args
                .next()
                .map(|value| {
                    value.parse::<u64>().map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "event duration is not an integer",
                        )
                    })
                })
                .transpose()?
                .unwrap_or(10);
            if args.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "events accepts DEVICE and optional SECONDS",
                ));
            }
            input::capture_events(Path::new(&device), Duration::from_secs(seconds))
        }
        Some("capture") => {
            let device = args.next().unwrap_or_else(|| DEFAULT_FRAMEBUFFER.into());
            if args.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "capture accepts at most one framebuffer path",
                ));
            }
            let mut stdout = io::stdout().lock();
            let info = framebuffer::capture_to(Path::new(&device), &mut stdout)?;
            eprintln!(
                "captured {}x{} {}bpp framebuffer from {}",
                info.width,
                info.height,
                info.bits_per_pixel,
                info.device.display()
            );
            Ok(())
        }
        Some("render-test") => {
            let device = args.next().unwrap_or_else(|| DEFAULT_FRAMEBUFFER.into());
            let seconds = args
                .next()
                .map(|value| {
                    value.parse::<u64>().map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "render-test wait duration is not an integer",
                        )
                    })
                })
                .transpose()?
                .unwrap_or(3);
            let waveform = args
                .next()
                .map(|value| WaveformMode::parse(&value))
                .transpose()?
                .unwrap_or(WaveformMode::Gc16);
            let wait_for_completion = args
                .next()
                .map(|value| match value.to_ascii_uppercase().as_str() {
                    "WAIT" => Ok(true),
                    "NOWAIT" => Ok(false),
                    _ => Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("unknown completion mode {value:?}; expected WAIT or NOWAIT"),
                    )),
                })
                .transpose()?
                .unwrap_or(true);
            if args.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "render-test accepts FRAMEBUFFER, optional SECONDS, optional WAVEFORM, and optional WAIT|NOWAIT",
                ));
            }
            let mut stdout = io::stdout().lock();
            framebuffer::render_test_to(
                Path::new(&device),
                Duration::from_secs(seconds),
                waveform,
                wait_for_completion,
                &mut stdout,
            )
        }
        Some("standalone-test") => {
            let device = args.next().unwrap_or_else(|| DEFAULT_FRAMEBUFFER.into());
            let suspend_mode = args
                .next()
                .map(|value| runtime::SuspendMode::parse(&value))
                .transpose()?
                .unwrap_or(runtime::SuspendMode::EInk);
            if args.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "standalone-test accepts FRAMEBUFFER and optional SUSPEND_MODE",
                ));
            }
            runtime::run(Path::new(&device), suspend_mode)
        }
        Some(command) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown command {command:?}; run `prs-t1-agent help`"),
        )),
    }
}

fn probe(device: &Path) {
    println!("PRS-T1 native inventory");
    match framebuffer::inspect(device) {
        Ok(info) => info.print(),
        Err(error) => println!("framebuffer_error={error}"),
    }
    android::print_inventory();
    input::print_inventory();
    status::print_status();
}

fn print_usage() {
    println!(
        "prs-t1-agent {}\n\nUsage:\n  prs-t1-agent probe [FRAMEBUFFER]\n  prs-t1-agent status\n  prs-t1-agent input\n  prs-t1-agent events [EVENT_DEVICE] [SECONDS]\n  prs-t1-agent capture [FRAMEBUFFER] > screen.pgm\n  prs-t1-agent render-test [FRAMEBUFFER] [SECONDS] [WAVEFORM] [WAIT|NOWAIT] > render-test.pgm\n  prs-t1-agent standalone-test [FRAMEBUFFER] [standby|mem]\n\n`probe`, `status`, `input`, `events`, and `capture` are read-only. `status`\nprints battery, power, USB, Wi-Fi, ADB, uptime, and Android-process state.\n`events` logs a bounded raw evdev stream without grabbing or injecting events.\n`capture` emits an 8-bit grayscale PGM. `render-test` is a write-capable\ncommand that briefly writes a centered RGB565 marker, requests a T1 e-ink\nupdate, captures the framebuffer, and restores the original rectangle. Its\noptional waveform is one of `DU`, `GC16`, `GC4`, or `A2`; it defaults to\n`GC16`. `NOWAIT` measures asynchronous submission and leaves completion to the\nbounded wait interval before the restore update; `WAIT` is the default.\n`standalone-test` is a long-running write-capable native UI test for use after\nstopping zygote; it holds a kernel wake lock, displays input data, sleeps on a\nshort power press, and reboots on a long power press. Its optional suspend mode\ndefaults to `standby`; `mem` selects Android's normal early-suspend path for\nwake testing.",
        env!("CARGO_PKG_VERSION")
    );
}

// The first T1 build uses the workspace's ARMv5 musl target so its static
// binary can run on the ARMv7 Android kernel without depending on Android's
// dynamic linker. That target needs these single-threaded bootstrap shims;
// replace them with proper ARM atomic support before adding worker threads.
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
