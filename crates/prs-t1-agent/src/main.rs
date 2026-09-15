mod android;
mod framebuffer;
mod input;

use std::env;
use std::io;
use std::path::Path;
use std::sync::atomic::{compiler_fence, Ordering};

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
}

fn print_usage() {
    println!(
        "prs-t1-agent {}\n\nUsage:\n  prs-t1-agent probe [FRAMEBUFFER]\n  prs-t1-agent input\n  prs-t1-agent capture [FRAMEBUFFER] > screen.pgm\n\nThe current commands are read-only. `probe` inventories framebuffer, Android\nprocess, and input state. `capture` emits an 8-bit grayscale PGM without\nwriting the framebuffer or issuing a display-refresh ioctl.",
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
