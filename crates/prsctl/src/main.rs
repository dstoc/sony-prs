mod error;

use crate::error::{Error, Result};
use scsi_transport::SgDevice;
use sony_x50::protocol::{Answer, Request};
use sony_x50::SonyExtendedTransport;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    match command.as_str() {
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        "version" | "--version" | "-V" => {
            println!("prsctl {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "probe" => {
            let sg_path = required(&mut args, "probe requires /dev/sgN")?;
            reject_extra(&mut args)?;
            probe(&sg_path)
        }
        "scan" => {
            reject_extra(&mut args)?;
            scan()
        }
        "get" => {
            let sg_path = required(&mut args, "get requires /dev/sgN")?;
            let remote_path = required(&mut args, "get requires a device path")?;
            let output_path = required(&mut args, "get requires an output path")?;
            reject_extra(&mut args)?;
            get(&sg_path, &remote_path, &output_path)
        }
        "size" => {
            let sg_path = required(&mut args, "size requires /dev/sgN")?;
            let remote_path = required(&mut args, "size requires a device path")?;
            reject_extra(&mut args)?;
            size(&sg_path, &remote_path)
        }
        "read" => {
            let sg_path = required(&mut args, "read requires /dev/sgN")?;
            let remote_path = required(&mut args, "read requires a device path")?;
            let offset = required(&mut args, "read requires an offset")?
                .parse::<u64>()
                .map_err(|_| Error::InvalidArgument("read offset is not an integer".into()))?;
            let count = required(&mut args, "read requires a byte count")?
                .parse::<usize>()
                .map_err(|_| Error::InvalidArgument("read count is not an integer".into()))?;
            reject_extra(&mut args)?;
            read(&sg_path, &remote_path, offset, count)
        }
        "dump" => {
            let sg_path = required(&mut args, "dump requires /dev/sgN")?;
            let remote_path = required(&mut args, "dump requires a device path")?;
            let output_path = required(&mut args, "dump requires an output path")?;
            let badmap_path = required(&mut args, "dump requires a bad-range map path")?;
            reject_extra(&mut args)?;
            dump(&sg_path, &remote_path, &output_path, &badmap_path)
        }
        "decode-answer" => {
            let path = required(&mut args, "decode-answer requires a packet file")?;
            reject_extra(&mut args)?;
            decode_answer(&path)
        }
        "decode-request" => {
            let path = required(&mut args, "decode-request requires a packet file")?;
            reject_extra(&mut args)?;
            decode_request(&path)
        }
        _ => Err(Error::InvalidArgument(format!(
            "unknown command {command:?}; run `prsctl help`"
        ))),
    }
}

fn probe(sg_path: &str) -> Result<()> {
    let device = SgDevice::open(sg_path)?;
    let inquiry = device.inquiry()?;
    println!("device: {}", device.path().display());
    println!("vendor: {}", inquiry.vendor);
    println!("product: {}", inquiry.product);
    println!("revision: {}", inquiry.revision);
    println!("peripheral type: 0x{:02x}", inquiry.peripheral_type);
    println!("Sony extended transport: verified for read-only x50 file access");
    Ok(())
}

fn scan() -> Result<()> {
    let mut paths = fs::read_dir("/dev")?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("sg"))
        })
        .collect::<Vec<_>>();
    if let Ok(entries) = fs::read_dir("/dev/bsg") {
        paths.extend(entries.filter_map(|entry| entry.ok().map(|entry| entry.path())));
    }
    paths.sort();
    if paths.is_empty() {
        println!("no /dev/sg* or /dev/bsg/* devices found");
        return Ok(());
    }
    for path in paths {
        match SgDevice::open(&path).and_then(|device| {
            let inquiry = device.inquiry()?;
            Ok(inquiry)
        }) {
            Ok(inquiry) => println!(
                "{}: {} {} {}",
                path.display(),
                inquiry.vendor,
                inquiry.product,
                inquiry.revision
            ),
            Err(error) => eprintln!("{}: {error}", path.display()),
        }
    }
    Ok(())
}

fn get(sg_path: &str, remote_path: &str, output_path: &str) -> Result<()> {
    let (mut transport, total_size, vendor, product) = open_transport(sg_path, remote_path)?;
    println!("{} {}", vendor, product);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)?;
    let mut offset = 0u64;
    let chunk_size = 0x1000usize;
    let mut recoveries = 0u32;
    while offset < total_size {
        let count = (total_size - offset).min(chunk_size as u64) as usize;
        match transport.file_read(remote_path, offset, count, total_size) {
            Ok(bytes) if !bytes.is_empty() => {
                output.write_all(&bytes).map_err(Error::from)?;
                offset += bytes.len() as u64;
                recoveries = 0;
                if offset % (1024 * 1024) < bytes.len() as u64 || offset == total_size {
                    eprintln!("progress: {offset}/{total_size} bytes");
                }
            }
            Ok(_) => {
                return Err(Error::Protocol(format!(
                    "Sony x50 Read returned no data at offset {offset} of {total_size}"
                )));
            }
            Err(error) => {
                recoveries += 1;
                if recoveries > 8 {
                    return Err(error.into());
                }
                eprintln!(
                    "read failed at offset {offset} ({error}); reconnecting ({recoveries}/8)"
                );
                let mut reconnected = None;
                let mut reconnect_error = None;
                for attempt in 1..=8 {
                    std::thread::sleep(Duration::from_secs(1));
                    match open_transport(sg_path, remote_path) {
                        Ok((candidate, size, _, _)) if size == total_size => {
                            reconnected = Some(candidate);
                            break;
                        }
                        Ok((_, size, _, _)) => {
                            reconnect_error = Some(Error::Protocol(format!(
                                "device size changed during read: {total_size} -> {size}"
                            )));
                        }
                        Err(error) => reconnect_error = Some(error),
                    }
                    eprintln!("reconnect attempt {attempt}/8 failed; retrying");
                }
                transport = reconnected.ok_or_else(|| {
                    reconnect_error.unwrap_or_else(|| {
                        Error::Protocol("could not reinitialize reader after failure".into())
                    })
                })?;
            }
        }
    }
    println!("read {offset} bytes from {remote_path} to {output_path}");
    Ok(())
}

fn open_transport(
    sg_path: &str,
    remote_path: &str,
) -> Result<(SonyExtendedTransport, u64, String, String)> {
    let device = SgDevice::open(sg_path)?;
    let inquiry = device.inquiry()?;
    let mut transport = SonyExtendedTransport::new(device);
    transport.initialize()?;
    let total_size = transport.file_size(remote_path)?;
    Ok((transport, total_size, inquiry.vendor, inquiry.product))
}

fn open_transport_with_retry(
    sg_path: &str,
    remote_path: &str,
    attempts: u32,
) -> Result<(SonyExtendedTransport, u64, String, String)> {
    let mut last_error = None;
    for attempt in 1..=attempts {
        match open_transport(sg_path, remote_path) {
            Ok(result) if result.1 != 0 => return Ok(result),
            Ok(_) => {
                last_error = Some(Error::Protocol(
                    "reader returned a zero-sized partition during connection".into(),
                ));
                if attempt != attempts {
                    eprintln!("connection attempt {attempt}/{attempts} returned size 0; retrying");
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            Err(error) => {
                last_error = Some(error);
                if attempt != attempts {
                    eprintln!("connection attempt {attempt}/{attempts} failed; retrying");
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| Error::Protocol("connection failed without an error".into())))
}

fn size(sg_path: &str, remote_path: &str) -> Result<()> {
    let device = SgDevice::open(sg_path)?;
    let inquiry = device.inquiry()?;
    let mut transport = SonyExtendedTransport::new(device);
    transport.initialize()?;
    let bytes = transport.file_size(remote_path)?;
    println!(
        "{} {}: {bytes} bytes for {remote_path}",
        inquiry.vendor, inquiry.product
    );
    Ok(())
}

fn read(sg_path: &str, remote_path: &str, offset: u64, count: usize) -> Result<()> {
    let device = SgDevice::open(sg_path)?;
    let inquiry = device.inquiry()?;
    let mut transport = SonyExtendedTransport::new(device);
    transport.initialize()?;
    let total_size = transport.file_size(remote_path)?;
    let bytes = transport.file_read(remote_path, offset, count, total_size)?;
    println!(
        "{} {}: read {} bytes at offset {} from {}",
        inquiry.vendor,
        inquiry.product,
        bytes.len(),
        offset,
        remote_path
    );
    println!("data: {}", hex_preview(&bytes, 64));
    Ok(())
}

fn dump(sg_path: &str, remote_path: &str, output_path: &str, badmap_path: &str) -> Result<()> {
    let (mut transport, total_size, vendor, product) =
        open_transport_with_retry(sg_path, remote_path, 8)?;
    println!("{} {}", vendor, product);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)?;
    let mut badmap = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(badmap_path)?;
    writeln!(badmap, "# remote={remote_path} size={total_size}").map_err(Error::from)?;

    let mut offset = 0u64;
    let chunk_size = 0x1000usize;
    while offset < total_size {
        let count = (total_size - offset).min(chunk_size as u64) as usize;
        match read_with_retry(
            &mut transport,
            sg_path,
            remote_path,
            offset,
            count,
            total_size,
            3,
        ) {
            Ok(bytes) => {
                output.write_all(&bytes).map_err(Error::from)?;
            }
            Err(error) => {
                eprintln!(
                    "chunk at offset {offset} remains unreadable ({error}); trying 512-byte sectors"
                );
                let mut suboffset = 0usize;
                while suboffset < count {
                    let subcount = (count - suboffset).min(512);
                    let absolute = offset + suboffset as u64;
                    match read_with_retry(
                        &mut transport,
                        sg_path,
                        remote_path,
                        absolute,
                        subcount,
                        total_size,
                        1,
                    ) {
                        Ok(bytes) => output.write_all(&bytes).map_err(Error::from)?,
                        Err(error) => {
                            eprintln!(
                                "unreadable range {absolute}..{} ({error}); zero-filling",
                                absolute + subcount as u64
                            );
                            writeln!(badmap, "{absolute} {} {error}", absolute + subcount as u64)
                                .map_err(Error::from)?;
                            output
                                .write_all(&vec![0u8; subcount])
                                .map_err(Error::from)?;
                        }
                    }
                    suboffset += subcount;
                }
            }
        }
        offset += count as u64;
        if offset % (1024 * 1024) < count as u64 || offset == total_size {
            eprintln!("progress: {offset}/{total_size} bytes");
        }
    }
    println!("dumped {offset} bytes from {remote_path} to {output_path}");
    Ok(())
}

fn read_with_retry(
    transport: &mut SonyExtendedTransport,
    sg_path: &str,
    remote_path: &str,
    offset: u64,
    count: usize,
    total_size: u64,
    max_attempts: u32,
) -> Result<Vec<u8>> {
    let mut last_error = None;
    for attempt in 1..=max_attempts {
        match transport.file_read(remote_path, offset, count, total_size) {
            Ok(bytes) if bytes.len() == count => return Ok(bytes),
            Ok(bytes) => {
                last_error = Some(Error::Protocol(format!(
                    "Sony x50 Read returned {} bytes, expected {count}",
                    bytes.len()
                )))
            }
            Err(error) => last_error = Some(error.into()),
        }
        eprintln!("read retry {attempt}/{max_attempts} at offset {offset}");
        std::thread::sleep(Duration::from_secs(1));
        match open_transport(sg_path, remote_path) {
            Ok((candidate, size, _, _)) if size == total_size => *transport = candidate,
            Ok((_, size, _, _)) => {
                last_error = Some(Error::Protocol(format!(
                    "device size changed during read: {total_size} -> {size}"
                )));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| Error::Protocol("read failed without an error".into())))
}

fn decode_answer(path: &str) -> Result<()> {
    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let answer = Answer::decode(&bytes)?;
    println!("data length: {}", answer.data.len());
    println!("data: {}", hex_preview(&answer.data, 64));
    Ok(())
}

fn decode_request(path: &str) -> Result<()> {
    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let request = Request::decode(&bytes)?;
    println!(
        "command: {} (0x{:x})",
        request.command.name(),
        request.command as u32
    );
    println!("extra length: {}", request.extra.len());
    println!("extra: {}", hex_preview(&request.extra, 64));
    Ok(())
}

fn required(args: &mut impl Iterator<Item = String>, message: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| Error::InvalidArgument(message.into()))
}

fn reject_extra(args: &mut impl Iterator<Item = String>) -> Result<()> {
    if let Some(argument) = args.next() {
        return Err(Error::InvalidArgument(format!(
            "unexpected argument {argument:?}"
        )));
    }
    Ok(())
}

fn hex_preview(bytes: &[u8], max: usize) -> String {
    let mut result = String::new();
    for (index, byte) in bytes.iter().take(max).enumerate() {
        if index != 0 {
            result.push(' ');
        }
        result.push_str(&format!("{byte:02x}"));
    }
    if bytes.len() > max {
        result.push_str(" …");
    }
    result
}

fn print_usage() {
    println!(
        "prsctl {}\n\nUsage:\n  prsctl scan\n  prsctl probe /dev/sgN\n  prsctl get /dev/sgN DEVICE_PATH OUTPUT\n  prsctl size /dev/sgN DEVICE_PATH\n  prsctl read /dev/sgN DEVICE_PATH OFFSET COUNT\n  prsctl dump /dev/sgN DEVICE_PATH OUTPUT BADMAP\n  prsctl decode-request PACKET\n  prsctl decode-answer PACKET\n\nThis command is limited to read-only SCSI extraction and offline packet decoding.\nOutput files are created exclusively and are never overwritten.",
        env!("CARGO_PKG_VERSION")
    );
}
