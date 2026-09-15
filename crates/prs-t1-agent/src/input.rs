use std::fs;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::raw::{c_int, c_ulong};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const EV_SYN: usize = 0;
const EV_ABS: usize = 3;
const ABS_MAX: usize = 64;
const EVENT_BITS_BYTES: usize = 8;

#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
struct InputAbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

pub fn print_inventory() {
    println!("input_nodes=");
    for index in 0..=15 {
        let path = PathBuf::from(format!("/dev/input/event{index}"));
        print_node(&path);
        if path.exists() {
            match inspect_event(&path) {
                Ok(info) => info.print(),
                Err(error) => println!("  ioctl_error={} error={error}", path.display()),
            }
        }
    }
    let subcpu = PathBuf::from("/dev/subcpu");
    if subcpu.exists() {
        print_node(&subcpu);
    } else {
        println!("  path=/dev/subcpu present=false");
    }

    println!("input_capabilities=");
    match fs::read_to_string("/proc/bus/input/devices") {
        Ok(content) => {
            for line in content.lines() {
                if line.starts_with("I: ")
                    || line.starts_with("N: ")
                    || line.starts_with("H: ")
                    || line.starts_with("B: EV=")
                    || line.starts_with("B: ABS=")
                    || line.starts_with("B: KEY=")
                {
                    println!("  {line}");
                }
            }
        }
        Err(error) => println!("  unavailable={error}"),
    }
}

fn print_node(path: &Path) {
    match fs::metadata(path) {
        Ok(metadata) => println!(
            "  path={} mode={:o}",
            path.display(),
            metadata.permissions().mode() & 0o777
        ),
        Err(error) => println!("  path={} unavailable={error}", path.display()),
    }
}

struct EventInfo {
    path: PathBuf,
    name: String,
    id: InputId,
    event_bits: [u8; EVENT_BITS_BYTES],
    abs_bits: [u8; EVENT_BITS_BYTES],
    axes: Vec<(usize, InputAbsInfo)>,
}

impl EventInfo {
    fn print(&self) {
        println!(
            "  event_info={} name={:?} id={:04x}:{:04x}:{:04x}:{:04x}",
            self.path.display(),
            self.name,
            self.id.bustype,
            self.id.vendor,
            self.id.product,
            self.id.version
        );
        println!("    ev_bits={}", format_bits(&self.event_bits));
        println!("    abs_bits={}", format_bits(&self.abs_bits));
        for (axis, info) in &self.axes {
            println!(
                "    abs{axis}=value:{} min:{} max:{} fuzz:{} flat:{} resolution:{}",
                info.value, info.minimum, info.maximum, info.fuzz, info.flat, info.resolution
            );
        }
    }
}

fn inspect_event(path: &Path) -> io::Result<EventInfo> {
    let file = File::open(path)?;
    let mut name = [0u8; 128];
    ioctl_read(&file, ev_ioc(0x06, name.len()), &mut name)?;
    let name = String::from_utf8_lossy(&name)
        .split('\0')
        .next()
        .unwrap_or_default()
        .to_string();

    let mut id = InputId::default();
    ioctl_read(&file, ev_ioc(0x02, std::mem::size_of::<InputId>()), &mut id)?;
    let mut event_bits = [0u8; EVENT_BITS_BYTES];
    ioctl_read(
        &file,
        ev_ioc(0x20 + EV_SYN, event_bits.len()),
        &mut event_bits,
    )?;
    let mut abs_bits = [0u8; EVENT_BITS_BYTES];
    ioctl_read(&file, ev_ioc(0x20 + EV_ABS, abs_bits.len()), &mut abs_bits)?;

    let mut axes = Vec::new();
    for axis in 0..ABS_MAX {
        if bit_is_set(&abs_bits, axis) {
            let mut info = InputAbsInfo::default();
            ioctl_read(
                &file,
                ev_ioc(0x40 + axis, std::mem::size_of::<InputAbsInfo>()),
                &mut info,
            )?;
            axes.push((axis, info));
        }
    }
    Ok(EventInfo {
        path: path.to_path_buf(),
        name,
        id,
        event_bits,
        abs_bits,
        axes,
    })
}

fn ioctl_read<T>(file: &File, request: c_ulong, value: &mut T) -> io::Result<()> {
    let result = unsafe { ioctl(file.as_raw_fd(), request, value) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn ev_ioc(number: usize, size: usize) -> c_ulong {
    (2u64 << 30 | (size as u64) << 16 | (u64::from(b'E')) << 8 | number as u64) as c_ulong
}

fn bit_is_set(bits: &[u8], bit: usize) -> bool {
    bits.get(bit / 8)
        .is_some_and(|value| value & (1 << (bit % 8)) != 0)
}

fn format_bits(bits: &[u8]) -> String {
    let mut values = Vec::new();
    for bit in 0..bits.len() * 8 {
        if bit_is_set(bits, bit) {
            values.push(bit.to_string());
        }
    }
    if values.is_empty() {
        "none".into()
    } else {
        values.join(",")
    }
}
