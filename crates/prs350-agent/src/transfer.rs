use super::*;

pub(super) fn receive_and_execute() {
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

pub(super) fn receive_and_shell() {
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

        let command =
            String::from_utf8(receive_payload(bytes, expected_crc, "SHELL")?).map_err(|_| {
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

pub(super) fn receive_payload_from(
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

pub(super) fn execute_binary(payload: &[u8]) -> io::Result<(usize, u32, u32)> {
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

pub(super) fn crc32_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    crc
}
