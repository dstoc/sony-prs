use std::fs;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::raw::{c_int, c_ulong};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const EV_SYN: usize = 0;
const EV_ABS: usize = 3;
const ABS_MAX: usize = 64;
const EVENT_BITS_BYTES: usize = 8;
const EVENT_SIZE: usize = 16;
const O_NONBLOCK: i32 = 0x800;

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

pub fn capture_events(path: &Path, duration: Duration) -> io::Result<()> {
    let mut reader = EventReader::open(path)?;
    println!("event_capture={}", path.display());
    println!("duration_seconds={}", duration.as_secs());
    println!("event_size={EVENT_SIZE}");
    println!("evdev_grab=false");
    println!("event_injection=false");

    let deadline = Instant::now() + duration;
    let mut count = 0usize;
    while Instant::now() < deadline {
        match reader.read_one()? {
            Some(event) => {
                println!(
                    "event index={} sec={} usec={} type={} code={} value={}",
                    count, event.sec, event.usec, event.event_type, event.code, event.value
                );
                count += 1;
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    }
    println!("event_count={count}");
    Ok(())
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

#[derive(Debug, Clone, Copy)]
pub struct RawEvent {
    pub sec: u32,
    pub usec: u32,
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}

impl RawEvent {
    pub fn timestamp_micros(self) -> u64 {
        u64::from(self.sec) * 1_000_000 + u64::from(self.usec)
    }
}

pub struct EventReader {
    file: File,
}

impl EventReader {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(O_NONBLOCK)
            .open(path)?;
        Ok(Self { file })
    }

    pub fn read_one(&mut self) -> io::Result<Option<RawEvent>> {
        let mut buffer = [0u8; EVENT_SIZE];
        match self.file.read(&mut buffer) {
            Ok(EVENT_SIZE) => Ok(Some(decode_event(&buffer))),
            Ok(0) => Ok(None),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "evdev returned a partial event",
            )),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn decode_event(bytes: &[u8; EVENT_SIZE]) -> RawEvent {
    RawEvent {
        sec: u32::from_ne_bytes(bytes[0..4].try_into().unwrap()),
        usec: u32::from_ne_bytes(bytes[4..8].try_into().unwrap()),
        event_type: u16::from_ne_bytes(bytes[8..10].try_into().unwrap()),
        code: u16::from_ne_bytes(bytes[10..12].try_into().unwrap()),
        value: i32::from_ne_bytes(bytes[12..16].try_into().unwrap()),
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_event, EVENT_SIZE};

    #[test]
    fn decodes_32_bit_linux_input_event_layout() {
        let mut bytes = [0u8; EVENT_SIZE];
        bytes[0..4].copy_from_slice(&123u32.to_ne_bytes());
        bytes[4..8].copy_from_slice(&456u32.to_ne_bytes());
        bytes[8..10].copy_from_slice(&1u16.to_ne_bytes());
        bytes[10..12].copy_from_slice(&ABS_X_CODE.to_ne_bytes());
        bytes[12..16].copy_from_slice(&789i32.to_ne_bytes());

        let event = decode_event(&bytes);
        assert_eq!(event.sec, 123);
        assert_eq!(event.usec, 456);
        assert_eq!(event.event_type, 1);
        assert_eq!(event.code, ABS_X_CODE);
        assert_eq!(event.value, 789);
    }

    const ABS_X_CODE: u16 = 0;
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
