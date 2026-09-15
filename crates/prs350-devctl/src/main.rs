mod error;
mod serial;

use crate::error::{Error, Result};
use serial::SerialClient;
use std::env;
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
            println!("prs350-devctl {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "ping" => {
            let path = required(&mut args, "ping requires /dev/ttyACM0")?;
            reject_extra(&mut args)?;
            let mut client = SerialClient::open(path)?;
            client.ping()?;
            println!("{}: PONG", client.path().display());
            Ok(())
        }
        "info" => {
            let path = required(&mut args, "info requires /dev/ttyACM0")?;
            reject_extra(&mut args)?;
            let mut client = SerialClient::open(path)?;
            let info = client.info()?;
            println!("{}: {info}", client.path().display());
            Ok(())
        }
        "status" => {
            let path = required(&mut args, "status requires /dev/ttyACM0")?;
            reject_extra(&mut args)?;
            let mut client = SerialClient::open(path)?;
            let status = client.status()?;
            println!("{}: {status}", client.path().display());
            Ok(())
        }
        "reboot" => {
            let path = required(&mut args, "reboot requires /dev/ttyACM0")?;
            reject_extra(&mut args)?;
            let mut client = SerialClient::open(path)?;
            client.reboot()?;
            println!("{}: reboot requested", client.path().display());
            Ok(())
        }
        "probe" => {
            let path = required(&mut args, "probe requires /dev/ttyACM0")?;
            reject_extra(&mut args)?;
            let mut client = SerialClient::open(path)?;
            let probe = client.probe()?;
            println!("{}: {probe}", client.path().display());
            Ok(())
        }
        "render" => {
            let path = required(&mut args, "render requires /dev/ttyACM0")?;
            reject_extra(&mut args)?;
            let mut client = SerialClient::open(path)?;
            let result = client.render()?;
            println!("{}: {result}", client.path().display());
            Ok(())
        }
        "screenshot" => {
            let path = required(&mut args, "screenshot requires /dev/ttyACM0")?;
            let output = required(&mut args, "screenshot requires an output .pgm path")?;
            reject_extra(&mut args)?;
            let mut client = SerialClient::open(path)?.with_timeout(Duration::from_secs(120));
            let header = client.screenshot(&output)?;
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
        "exec" => {
            let path = required(&mut args, "exec requires /dev/ttyACM0")?;
            let binary = required(&mut args, "exec requires an ARM binary path")?;
            reject_extra(&mut args)?;
            let mut client = SerialClient::open(path)?.with_timeout(Duration::from_secs(180));
            let result = client.execute(binary)?;
            println!("{}: {result}", client.path().display());
            Ok(())
        }
        "shell" => {
            let path = required(&mut args, "shell requires /dev/ttyACM0")?;
            let command = required(&mut args, "shell requires a command")?;
            reject_extra(&mut args)?;
            let mut client = SerialClient::open(path)?.with_timeout(Duration::from_secs(30));
            let result = client.shell(&command)?;
            println!("{}: {}", client.path().display(), result.status);
            std::io::Write::write_all(&mut std::io::stdout(), &result.output)?;
            Ok(())
        }
        _ => Err(Error::InvalidArgument(format!(
            "unknown command {command:?}; run `prs350-devctl help`"
        ))),
    }
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

fn print_usage() {
    println!(
        "prs350-devctl {}\n\nUsage:\n  prs350-devctl ping /dev/ttyACM0\n  prs350-devctl info /dev/ttyACM0\n  prs350-devctl status /dev/ttyACM0\n  prs350-devctl reboot /dev/ttyACM0\n  prs350-devctl probe /dev/ttyACM0\n  prs350-devctl render /dev/ttyACM0\n  prs350-devctl screenshot /dev/ttyACM0 OUTPUT.pgm\n  prs350-devctl exec /dev/ttyACM0 ARM_BINARY\n  prs350-devctl shell /dev/ttyACM0 COMMAND\n\nThis is an explicitly stateful PRS-350 development tool. It can reboot the\nreader, render to its framebuffer, execute an ARM binary, and run shell\ncommands. Output files are created exclusively and are never overwritten.",
        env!("CARGO_PKG_VERSION")
    );
}
