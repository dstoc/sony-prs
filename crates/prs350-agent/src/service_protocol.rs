use super::*;

pub(super) use prs350_wire::Request as ServiceRequest;

pub(super) fn parse_service_request(line: &[u8]) -> io::Result<ServiceRequest> {
    prs350_wire::decode_request(line)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))
}

pub(super) fn read_protocol_line(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match input.read(&mut byte)? {
            0 => return Ok(if line.is_empty() { None } else { Some(line) }),
            1 => {
                line.push(byte[0]);
                if line.len() > MAX_PROTOCOL_LINE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "serial request line is too long",
                    ));
                }
                if byte[0] == b'\n' {
                    return Ok(Some(line));
                }
            }
            _ => unreachable!(),
        }
    }
}

pub(super) fn write_protocol_line(output: &mut impl Write, line: &str) -> io::Result<()> {
    if line.len() + 1 > MAX_PROTOCOL_LINE || line.bytes().any(|byte| byte == b'\r' || byte == b'\n')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "serial response line is invalid",
        ));
    }
    output.write_all(line.as_bytes())?;
    output.write_all(b"\n")?;
    output.flush()
}

pub(super) fn write_protocol_error(output: &mut impl Write, error: &io::Error) -> io::Result<()> {
    let message = error.to_string().replace(['\r', '\n'], " ");
    write_protocol_line(output, &format!("PRS1 ERR {message}"))
}
