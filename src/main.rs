use prsctl::error::{Error, Result};
use prsctl::protocol::{Answer, Request};
use prsctl::serial::SerialClient;
use prsctl::sg::SgDevice;
use prsctl::SonyExtendedTransport;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Read;
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
        "decode-answer" => {
            let path = required(&mut args, "decode-answer requires a packet file")?;
            reject_extra(&mut args)?;
            decode_answer(&path)
        }
        "serial-ping" => {
            let path = required(&mut args, "serial-ping requires /dev/ttyACM0")?;
            reject_extra(&mut args)?;
            serial_ping(&path)
        }
        "serial-info" => {
            let path = required(&mut args, "serial-info requires /dev/ttyACM0")?;
            reject_extra(&mut args)?;
            serial_info(&path)
        }
        "serial-status" => {
            let path = required(&mut args, "serial-status requires /dev/ttyACM0")?;
            reject_extra(&mut args)?;
            serial_status(&path)
        }
        "serial-reboot" => {
            let path = required(&mut args, "serial-reboot requires /dev/ttyACM0")?;
            reject_extra(&mut args)?;
            serial_reboot(&path)
        }
        "serial-probe" => {
            let path = required(&mut args, "serial-probe requires /dev/ttyACM0")?;
            reject_extra(&mut args)?;
            serial_probe(&path)
        }
        "serial-render" => {
            let path = required(&mut args, "serial-render requires /dev/ttyACM0")?;
            reject_extra(&mut args)?;
            serial_render(&path)
        }
        "serial-screenshot" => {
            let path = required(&mut args, "serial-screenshot requires /dev/ttyACM0")?;
            let output = required(&mut args, "serial-screenshot requires an output .pgm path")?;
            reject_extra(&mut args)?;
            serial_screenshot(&path, &output)
        }
        "serial-exec" => {
            let path = required(&mut args, "serial-exec requires /dev/ttyACM0")?;
            let binary = required(&mut args, "serial-exec requires an ARM binary path")?;
            reject_extra(&mut args)?;
            serial_exec(&path, &binary)
        }
        "serial-shell" => {
            let path = required(&mut args, "serial-shell requires /dev/ttyACM0")?;
            let command = required(&mut args, "serial-shell requires a command")?;
            reject_extra(&mut args)?;
            serial_shell(&path, &command)
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
    paths.sort();
    if paths.is_empty() {
        println!("no /dev/sg* devices found");
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
    let device = SgDevice::open(sg_path)?;
    let inquiry = device.inquiry()?;
    println!("{} {}", inquiry.vendor, inquiry.product);
    let mut transport = SonyExtendedTransport::new(device);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)?;
    let bytes = transport.copy_file(remote_path, &mut output)?;
    println!("read {bytes} bytes from {remote_path} to {output_path}");
    Ok(())
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

fn serial_ping(path: &str) -> Result<()> {
    let mut client = SerialClient::open(path)?;
    client.ping()?;
    println!("{}: PONG", client.path().display());
    Ok(())
}

fn serial_info(path: &str) -> Result<()> {
    let mut client = SerialClient::open(path)?;
    let info = client.info()?;
    println!("{}: {}", client.path().display(), info);
    Ok(())
}

fn serial_status(path: &str) -> Result<()> {
    let mut client = SerialClient::open(path)?;
    let status = client.status()?;
    println!("{}: {}", client.path().display(), status);
    Ok(())
}

fn serial_reboot(path: &str) -> Result<()> {
    let mut client = SerialClient::open(path)?;
    client.reboot()?;
    println!("{}: reboot requested", client.path().display());
    Ok(())
}

fn serial_probe(path: &str) -> Result<()> {
    let mut client = SerialClient::open(path)?;
    let probe = client.probe()?;
    println!("{}: {}", client.path().display(), probe);
    Ok(())
}

fn serial_render(path: &str) -> Result<()> {
    let mut client = SerialClient::open(path)?;
    let result = client.render()?;
    println!("{}: {}", client.path().display(), result);
    Ok(())
}

fn serial_screenshot(path: &str, output: &str) -> Result<()> {
    let mut client = SerialClient::open(path)?.with_timeout(Duration::from_secs(120));
    let header = client.screenshot(output)?;
    println!(
        "{}: captured {}x{} {} to {}",
        client.path().display(),
        header.width,
        header.height,
        header.format,
        output
    );
    Ok(())
}

fn serial_exec(path: &str, binary: &str) -> Result<()> {
    let mut client = SerialClient::open(path)?.with_timeout(Duration::from_secs(180));
    let result = client.execute(binary)?;
    println!("{}: {}", client.path().display(), result);
    Ok(())
}

fn serial_shell(path: &str, command: &str) -> Result<()> {
    let mut client = SerialClient::open(path)?.with_timeout(Duration::from_secs(30));
    let result = client.shell(command)?;
    println!("{}: {}", client.path().display(), result.status);
    std::io::Write::write_all(&mut std::io::stdout(), &result.output)?;
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
        "prsctl {}\n\nUsage:\n  prsctl scan\n  prsctl probe /dev/sgN\n  prsctl get /dev/sgN DEVICE_PATH OUTPUT\n  prsctl serial-ping /dev/ttyACM0\n  prsctl serial-info /dev/ttyACM0\n  prsctl serial-status /dev/ttyACM0\n  prsctl serial-reboot /dev/ttyACM0\n  prsctl serial-probe /dev/ttyACM0\n  prsctl serial-render /dev/ttyACM0\n  prsctl serial-screenshot /dev/ttyACM0 OUTPUT.pgm\n  prsctl serial-exec /dev/ttyACM0 ARM_BINARY\n  prsctl serial-shell /dev/ttyACM0 COMMAND\n  prsctl decode-request PACKET\n  prsctl decode-answer PACKET\n\nThe tool is read-only except for the explicit serial-reboot, serial-render,\nserial-exec, and serial-shell commands. Screenshot and other output files are created exclusively\nand are never overwritten.",
        env!("CARGO_PKG_VERSION")
    );
}
