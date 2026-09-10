use crate::error::{Error, Result};

pub const REQUEST_HEADER_LEN: usize = 16;
pub const ANSWER_HEADER_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ReadCommand {
    FileOpen = 0x10,
    FileClose = 0x11,
    GetSize = 0x12,
    SetPosition = 0x14,
    GetPosition = 0x15,
    FileRead = 0x16,
    DirectoryIteratorNew = 0x33,
    DirectoryIteratorDispose = 0x34,
    DirectoryIteratorGetNext = 0x35,
    GetProperty = 0x101,
    GetMediaInfo = 0x102,
    GetFreeSpace = 0x103,
}

impl ReadCommand {
    pub fn from_id(id: u32) -> Result<Self> {
        match id {
            0x10 => Ok(Self::FileOpen),
            0x11 => Ok(Self::FileClose),
            0x12 => Ok(Self::GetSize),
            0x14 => Ok(Self::SetPosition),
            0x15 => Ok(Self::GetPosition),
            0x16 => Ok(Self::FileRead),
            0x33 => Ok(Self::DirectoryIteratorNew),
            0x34 => Ok(Self::DirectoryIteratorDispose),
            0x35 => Ok(Self::DirectoryIteratorGetNext),
            0x101 => Ok(Self::GetProperty),
            0x102 => Ok(Self::GetMediaInfo),
            0x103 => Ok(Self::GetFreeSpace),
            0x13 | 0x17 | 0x1a | 0x1b | 0x1c | 0x30 | 0x31 | 0x32 | 0x300..=0x304 => Err(
                Error::Protocol(format!("mutating command 0x{id:x} is disabled")),
            ),
            _ => Err(Error::Protocol(format!("unsupported command 0x{id:x}"))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::FileOpen => "FileOpen",
            Self::FileClose => "FileClose",
            Self::GetSize => "GetSize",
            Self::SetPosition => "SetPosition",
            Self::GetPosition => "GetPosition",
            Self::FileRead => "FileRead",
            Self::DirectoryIteratorNew => "DirectoryIteratorNew",
            Self::DirectoryIteratorDispose => "DirectoryIteratorDispose",
            Self::DirectoryIteratorGetNext => "DirectoryIteratorGetNext",
            Self::GetProperty => "GetProperty",
            Self::GetMediaInfo => "GetMediaInfo",
            Self::GetFreeSpace => "GetFreeSpace",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub command: ReadCommand,
    pub extra: Vec<u8>,
}

impl Request {
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_request(self.command, &self.extra)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < REQUEST_HEADER_LEN {
            return Err(Error::Protocol(format!(
                "request is {} bytes, need at least {REQUEST_HEADER_LEN}",
                bytes.len()
            )));
        }
        let command = ReadCommand::from_id(read_u32(bytes, 0)?)?;
        let extra_len = read_u32(bytes, 12)? as usize;
        let end = REQUEST_HEADER_LEN
            .checked_add(extra_len)
            .ok_or_else(|| Error::Protocol("request length overflows usize".into()))?;
        if end > bytes.len() {
            return Err(Error::Protocol(format!(
                "request declares {extra_len} extra bytes, but only {} are present",
                bytes.len() - REQUEST_HEADER_LEN
            )));
        }
        Ok(Self {
            command,
            extra: bytes[REQUEST_HEADER_LEN..end].to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub data: Vec<u8>,
}

impl Answer {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < ANSWER_HEADER_LEN {
            return Err(Error::Protocol(format!(
                "answer is {} bytes, need at least {ANSWER_HEADER_LEN}",
                bytes.len()
            )));
        }

        let data_len = read_u32(bytes, 12)? as usize;
        let end = ANSWER_HEADER_LEN
            .checked_add(data_len)
            .ok_or_else(|| Error::Protocol("answer length overflows usize".into()))?;
        if end > bytes.len() {
            return Err(Error::Protocol(format!(
                "answer declares {data_len} data bytes, but only {} are present",
                bytes.len() - ANSWER_HEADER_LEN
            )));
        }

        Ok(Self {
            data: bytes[ANSWER_HEADER_LEN..end].to_vec(),
        })
    }
}

pub fn encode_request(command: ReadCommand, extra: &[u8]) -> Result<Vec<u8>> {
    let extra_len = u32::try_from(extra.len())
        .map_err(|_| Error::InvalidArgument("request payload is too large".into()))?;
    let mut result = Vec::with_capacity(REQUEST_HEADER_LEN + extra.len());
    put_u32(&mut result, command as u32);
    put_u32(&mut result, 0);
    put_u32(&mut result, 0);
    put_u32(&mut result, extra_len);
    result.extend_from_slice(extra);
    Ok(result)
}

pub fn encode_file_open(path: &str) -> Result<Vec<u8>> {
    let extra = file_open_extra(path)?;
    encode_request(ReadCommand::FileOpen, &extra)
}

pub fn file_open_extra(path: &str) -> Result<Vec<u8>> {
    let path_bytes = path.as_bytes();
    if path_bytes.contains(&0) {
        return Err(Error::InvalidArgument("device path contains NUL".into()));
    }
    let path_len = u32::try_from(path_bytes.len())
        .map_err(|_| Error::InvalidArgument("device path is too long".into()))?;
    // FileOpen carries a read/write mode before the path length. This client
    // is deliberately read-only, so mode is always zero.
    let mut extra = Vec::with_capacity(8 + path_bytes.len());
    put_u32(&mut extra, 0);
    put_u32(&mut extra, path_len);
    extra.extend_from_slice(path_bytes);
    Ok(extra)
}

pub fn encode_handle(handle: u32) -> Vec<u8> {
    let mut extra = Vec::with_capacity(4);
    put_u32(&mut extra, handle);
    extra
}

pub fn encode_file_read(handle: u32, count: usize) -> Result<Vec<u8>> {
    let extra = file_read_extra(handle, count)?;
    encode_request(ReadCommand::FileRead, &extra)
}

pub fn file_read_extra(handle: u32, count: usize) -> Result<Vec<u8>> {
    let count = u32::try_from(count)
        .map_err(|_| Error::InvalidArgument("read size is too large".into()))?;
    let mut extra = encode_handle(handle);
    put_u32(&mut extra, count);
    Ok(extra)
}

pub fn encode_set_position(handle: u32, position: u64) -> Result<Vec<u8>> {
    let extra = set_position_extra(handle, position);
    encode_request(ReadCommand::SetPosition, &extra)
}

pub fn set_position_extra(handle: u32, position: u64) -> Vec<u8> {
    let mut extra = encode_handle(handle);
    put_u64(&mut extra, position);
    extra
}

pub fn decode_handle(data: &[u8]) -> Result<u32> {
    if data.len() != 4 {
        return Err(Error::Protocol(format!(
            "file handle answer is {} bytes, expected 4",
            data.len()
        )));
    }
    read_u32(data, 0)
}

pub fn decode_integer(data: &[u8], field: &str) -> Result<u64> {
    match data.len() {
        4 => Ok(read_u32(data, 0)? as u64),
        8 => read_u64(data, 0),
        length => Err(Error::Protocol(format!(
            "{field} answer is {length} bytes, expected 4 or 8"
        ))),
    }
}

fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| Error::Protocol("integer offset overflows usize".into()))?;
    let slice = bytes
        .get(offset..end)
        .ok_or_else(|| Error::Protocol("short integer field".into()))?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("slice length checked"),
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| Error::Protocol("integer offset overflows usize".into()))?;
    let slice = bytes
        .get(offset..end)
        .ok_or_else(|| Error::Protocol("short integer field".into()))?;
    Ok(u64::from_le_bytes(
        slice.try_into().expect("slice length checked"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_header_uses_little_endian_fields() {
        let bytes = encode_request(ReadCommand::GetSize, &[1, 2, 3]).unwrap();
        assert_eq!(
            bytes,
            vec![0x12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 1, 2, 3]
        );
    }

    #[test]
    fn answer_length_is_checked_before_slicing() {
        let mut bytes = vec![0; ANSWER_HEADER_LEN];
        bytes[12..16].copy_from_slice(&4u32.to_le_bytes());
        assert!(Answer::decode(&bytes).is_err());

        bytes.extend_from_slice(&[9, 8, 7, 6]);
        assert_eq!(Answer::decode(&bytes).unwrap().data, vec![9, 8, 7, 6]);
    }

    #[test]
    fn file_open_has_read_mode_and_length_prefixed_path() {
        let bytes = encode_file_open("/Data/tmp/info/model").unwrap();
        assert_eq!(&bytes[0..4], &(ReadCommand::FileOpen as u32).to_le_bytes());
        assert_eq!(&bytes[16..20], &0u32.to_le_bytes());
        assert_eq!(&bytes[20..24], &20u32.to_le_bytes());
        assert_eq!(&bytes[24..], b"/Data/tmp/info/model");
    }

    #[test]
    fn file_open_rejects_nul() {
        assert!(encode_file_open("/Data/\0/model").is_err());
    }

    #[test]
    fn request_decoder_rejects_mutating_command_ids() {
        let mut bytes = vec![0; REQUEST_HEADER_LEN];
        bytes[0..4].copy_from_slice(&0x17u32.to_le_bytes());
        assert!(Request::decode(&bytes).is_err());
    }
}
