use super::framebuffer::{self, probe_framebuffer, probe_input, render_test};
use super::service_protocol::{self, ServiceRequest};
use super::transfer;
use super::*;

pub(super) fn run_serial_service() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let result = run_serial_service_with(stdin.lock(), stdout.lock());
    if let Err(error) = result {
        eprintln!("Rust serial service stopped: {error}");
    }
}

fn run_serial_service_with(mut input: impl Read, mut output: impl Write) -> io::Result<()> {
    loop {
        let Some(line) = service_protocol::read_protocol_line(&mut input)? else {
            return Ok(());
        };
        let request = match service_protocol::parse_service_request(&line) {
            Ok(request) => request,
            Err(error) => {
                service_protocol::write_protocol_error(&mut output, &error)?;
                continue;
            }
        };

        let result = handle_service_request(request, &mut input, &mut output);
        if let Err(error) = result {
            service_protocol::write_protocol_error(&mut output, &error)?;
        }
    }
}

fn handle_service_request(
    request: ServiceRequest,
    input: &mut impl Read,
    output: &mut impl Write,
) -> io::Result<()> {
    match request {
        ServiceRequest::Ping => service_protocol::write_protocol_line(output, "PRS1 OK PONG"),
        ServiceRequest::Info => service_protocol::write_protocol_line(
            output,
            "PRS1 OK INFO model=PRS-350 transport=cdc-acm",
        ),
        ServiceRequest::Status => {
            let ui = if stock_ui_alive() {
                "stock-alive"
            } else {
                "stock-missing"
            };
            service_protocol::write_protocol_line(output, &format!("PRS1 OK STATUS ui={ui}"))
        }
        ServiceRequest::Reboot => {
            service_protocol::write_protocol_line(output, "PRS1 OK REBOOTING")?;
            let _ = Command::new("/bin/sync").status();
            std::thread::sleep(Duration::from_secs(1));
            let _ = Command::new("/sbin/reboot").status();
            Ok(())
        }
        ServiceRequest::Probe => service_protocol::write_protocol_line(
            output,
            &format!(
                "PRS1 OK PROBE fb={} input={}",
                probe_framebuffer(),
                probe_input()
            ),
        ),
        ServiceRequest::Render => service_protocol::write_protocol_line(
            output,
            &format!("PRS1 OK RENDERED {}", render_test()),
        ),
        ServiceRequest::Capture => framebuffer::capture_framebuffer_to(output),
        ServiceRequest::Execute { bytes, crc32 } => {
            set_serial_mode(true)?;
            service_protocol::write_protocol_line(output, "PRS1 READY EXEC")?;
            let payload_result =
                transfer::receive_payload_from(input, output, bytes as usize, crc32, "EXEC");
            set_serial_mode(false)?;
            let payload = payload_result?;
            let (size, crc, pid) = transfer::execute_binary(&payload)?;
            service_protocol::write_protocol_line(
                output,
                &format!("PRS1 OK EXEC size={size} crc32={crc:08x} pid={pid}"),
            )
        }
        ServiceRequest::Shell { bytes, crc32 } => {
            set_serial_mode(true)?;
            service_protocol::write_protocol_line(output, "PRS1 READY SHELL")?;
            let payload_result =
                transfer::receive_payload_from(input, output, bytes as usize, crc32, "SHELL");
            set_serial_mode(false)?;
            let command = String::from_utf8(payload_result?).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "shell command is not UTF-8")
            })?;
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
            service_protocol::write_protocol_line(
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

pub(super) fn capture_framebuffer() {
    let mut stdout = io::stdout().lock();
    let result = framebuffer::capture_framebuffer_to(&mut stdout);
    if let Err(error) = result {
        let _ = service_protocol::write_protocol_error(
            &mut stdout,
            &io::Error::new(error.kind(), format!("capture-failed-{error}")),
        );
    }
}

pub(super) fn stock_ui_alive() -> bool {
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
        .any(|cmdline| {
            cmdline
                .windows(b"tinyhttp".len())
                .any(|part| part == b"tinyhttp")
        })
}
