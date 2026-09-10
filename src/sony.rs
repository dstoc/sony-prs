use crate::error::{Error, Result};
use crate::sg::{DataDirection, SgDevice};
use std::io::Write;

pub const SC_SONY_EXTENDED: u8 = 0x20;
pub const DEFAULT_BUFFER_SIZE: usize = 4096;

/// x50 transport for the Sony vendor SCSI opcode.
///
/// The recovered Sony helper uses a 10-byte CDB. CDB byte 2 selects the
/// command, while byte 4 selects the phase. The public file methods below
/// implement the read-only commands used by that helper.
#[derive(Debug)]
pub struct SonyExtendedTransport {
    device: SgDevice,
    max_transfer: usize,
}

impl SonyExtendedTransport {
    pub fn new(device: SgDevice) -> Self {
        Self {
            device,
            max_transfer: DEFAULT_BUFFER_SIZE,
        }
    }

    pub fn with_max_transfer(mut self, max_transfer: usize) -> Result<Self> {
        if max_transfer == 0 || max_transfer > u32::MAX as usize {
            return Err(Error::InvalidArgument(
                "maximum Sony transfer must be between 1 and 4 GiB".into(),
            ));
        }
        self.max_transfer = max_transfer;
        Ok(self)
    }

    pub fn device(&self) -> &SgDevice {
        &self.device
    }

    /// Exchange an x50 command using the framing used by Sony's helper DLL.
    ///
    /// The helper sends the command number in CDB byte 2 and uses separate
    /// phase markers for the host-to-device and device-to-host transfers.
    /// `request` and the returned bytes are command payloads; neither includes
    /// the legacy 16-byte Request/Answer header.
    pub fn exchange_command_bytes(
        &mut self,
        command: u8,
        request: &[u8],
        response_capacity: usize,
    ) -> Result<Vec<u8>> {
        if request.len() > self.max_transfer {
            return Err(Error::InvalidArgument(format!(
                "request is {} bytes, maximum is {}",
                request.len(),
                self.max_transfer
            )));
        }
        if response_capacity == 0 || response_capacity > self.max_transfer {
            return Err(Error::InvalidArgument(format!(
                "response capacity must be between 1 and {}",
                self.max_transfer
            )));
        }

        self.send_command_bytes(command, request)?;

        let mut response_data = vec![0u8; response_capacity];
        let cdb = Self::cdb(command, 2);
        let response = self
            .device
            .execute(&cdb, DataDirection::FromDevice, &mut response_data)?;
        response_data.truncate(response.bytes_transferred);
        Ok(response_data)
    }

    /// Send the host-to-device phase of an x50 command.
    pub fn send_command_bytes(&mut self, command: u8, request: &[u8]) -> Result<()> {
        if request.len() > self.max_transfer {
            return Err(Error::InvalidArgument(format!(
                "request is {} bytes, maximum is {}",
                request.len(),
                self.max_transfer
            )));
        }
        let mut request_data = request.to_vec();
        let cdb = Self::cdb(command, 1);
        self.device
            .execute(&cdb, DataDirection::ToDevice, &mut request_data)?;
        Ok(())
    }

    /// Receive a Sony command response without a preceding host-to-device
    /// phase. The x50 initialization command uses this form.
    pub fn receive_command_bytes(
        &mut self,
        command: u8,
        phase: u8,
        response_capacity: usize,
    ) -> Result<Vec<u8>> {
        if response_capacity == 0 || response_capacity > self.max_transfer {
            return Err(Error::InvalidArgument(format!(
                "response capacity must be between 1 and {}",
                self.max_transfer
            )));
        }

        let cdb = Self::cdb(command, phase);
        let mut response_data = vec![0u8; response_capacity];
        let response = self
            .device
            .execute(&cdb, DataDirection::FromDevice, &mut response_data)?;
        response_data.truncate(response.bytes_transferred);
        Ok(response_data)
    }

    /// Initialize the x50 file service for this reader.
    pub fn initialize(&mut self) -> Result<()> {
        let response = self.receive_command_bytes(0x06, 1, 0x15c)?;
        if response.len() != 0x15c || !response.starts_with(b"Reader\0") {
            return Err(Error::Protocol(format!(
                "unexpected x50 initialization response ({} bytes)",
                response.len()
            )));
        }
        Ok(())
    }

    /// Return the size of a reader-side file or device node.
    pub fn file_size(&mut self, path: &str) -> Result<u64> {
        let request = path_request(path)?;
        let response = self.exchange_command_bytes(0x80, &request, 4)?;
        let status = signed_status(&response, "GetSize")?;
        if status < 0 {
            return Err(Error::Protocol(format!(
                "Sony x50 GetSize returned status {status} for {path:?}"
            )));
        }
        Ok(status as u64)
    }

    /// Read a chunk from a reader-side file or device node.
    pub fn file_read(
        &mut self,
        path: &str,
        offset: u64,
        count: usize,
        total_size: u64,
    ) -> Result<Vec<u8>> {
        if count == 0 {
            return Err(Error::InvalidArgument("read size must be non-zero".into()));
        }
        let offset = u32::try_from(offset)
            .map_err(|_| Error::InvalidArgument("file offset exceeds 32-bit x50 limit".into()))?;
        let count_u32 = u32::try_from(count)
            .map_err(|_| Error::InvalidArgument("read size exceeds 32-bit x50 limit".into()))?;
        let total_size = u32::try_from(total_size)
            .map_err(|_| Error::InvalidArgument("file size exceeds 32-bit x50 limit".into()))?;
        let end = offset
            .checked_add(count_u32)
            .ok_or_else(|| Error::InvalidArgument("file read range overflows".into()))?;
        if end > total_size {
            return Err(Error::InvalidArgument(format!(
                "read range {offset}..{end} exceeds file size {total_size}"
            )));
        }

        let path_bytes = path_request(path)?;
        let mut request = vec![0u8; 0x10c];
        request[..path_bytes.len()].copy_from_slice(&path_bytes);
        request[0x100..0x104].copy_from_slice(&offset.to_le_bytes());
        request[0x104..0x108].copy_from_slice(&count_u32.to_le_bytes());
        request[0x108..0x10c].copy_from_slice(&total_size.to_le_bytes());
        self.send_command_bytes(0x81, &request)?;

        // The helper receives a four-byte status first. A successful read is
        // then returned in one or more data phases, with phase 2 marking the
        // final chunk and phase 3 marking intermediate chunks.
        let status_response = self.receive_command_bytes(0x81, 3, 4)?;
        let status = signed_status(&status_response, "Read")?;
        if status != 0 {
            return Err(Error::Protocol(format!(
                "Sony x50 Read returned status {status} for {path:?}"
            )));
        }

        let mut result = Vec::with_capacity(count);
        let mut remaining = count;
        while remaining != 0 {
            let chunk = remaining.min(self.max_transfer).min(0x1000);
            let phase = if chunk == remaining { 2 } else { 3 };
            let response = self.receive_command_bytes(0x81, phase, chunk)?;
            if response.len() != chunk {
                return Err(Error::Protocol(format!(
                    "Sony x50 Read returned {} bytes, expected {chunk}",
                    response.len()
                )));
            }
            result.extend_from_slice(&response);
            remaining -= chunk;
        }
        Ok(result)
    }

    /// Initialize the reader and copy a reader-side file to a local writer.
    pub fn copy_file<W: Write>(&mut self, path: &str, output: &mut W) -> Result<u64> {
        self.initialize()?;
        let total_size = self.file_size(path)?;
        let mut offset = 0u64;
        let chunk_size = self.max_transfer.min(0x1000);
        while offset < total_size {
            let count = (total_size - offset).min(chunk_size as u64) as usize;
            let bytes = self.file_read(path, offset, count, total_size)?;
            if bytes.is_empty() {
                return Err(Error::Protocol(format!(
                    "Sony x50 Read returned no data at offset {offset} of {total_size}"
                )));
            }
            output.write_all(&bytes).map_err(Error::from)?;
            offset += bytes.len() as u64;
        }
        Ok(offset)
    }

    fn cdb(command: u8, phase: u8) -> [u8; 10] {
        [SC_SONY_EXTENDED, 0, command, 0, phase, 0, 0, 0, 0, 0]
    }
}

fn path_request(path: &str) -> Result<Vec<u8>> {
    let path_bytes = path.as_bytes();
    if path_bytes.contains(&0) {
        return Err(Error::InvalidArgument("device path contains NUL".into()));
    }
    if path_bytes.len() >= 256 {
        return Err(Error::InvalidArgument(
            "x50 device path must fit in 255 bytes".into(),
        ));
    }
    let mut request = vec![0u8; 256];
    request[..path_bytes.len()].copy_from_slice(path_bytes);
    Ok(request)
}

fn signed_status(response: &[u8], command: &str) -> Result<i32> {
    let bytes = response
        .get(..4)
        .ok_or_else(|| Error::Protocol(format!("x50 {command} response is too short")))?;
    Ok(i32::from_le_bytes(
        bytes.try_into().expect("slice length checked"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x50_paths_are_nul_padded_to_256_bytes() {
        let request = path_request("/dev/mtdblock1").unwrap();
        assert_eq!(request.len(), 256);
        assert_eq!(&request[..14], b"/dev/mtdblock1");
        assert!(request[14..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn x50_paths_reject_nul_and_overflow() {
        assert!(path_request("/dev/\0mtdblock1").is_err());
        assert!(path_request(&"x".repeat(256)).is_err());
    }

    #[test]
    fn x50_status_is_signed_little_endian() {
        assert_eq!(
            signed_status(&[0xf0, 0xff, 0xff, 0xff], "test").unwrap(),
            -16
        );
    }
}
