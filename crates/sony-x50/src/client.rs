use crate::error::{Error, Result};
use crate::protocol::{
    decode_handle, decode_integer, encode_handle, file_open_extra, file_read_extra,
    set_position_extra, Answer, ReadCommand, Request, ANSWER_HEADER_LEN,
};
use std::io::Write;

pub trait PacketTransport {
    fn exchange(&mut self, request: &Request, max_response: usize) -> Result<Answer>;
}

pub struct ReaderClient<T> {
    transport: T,
    max_response: usize,
}

impl<T: PacketTransport> ReaderClient<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            max_response: 4096,
        }
    }

    pub fn with_max_response(mut self, max_response: usize) -> Result<Self> {
        if max_response <= 16 {
            return Err(Error::InvalidArgument(
                "maximum response must exceed the 16-byte Sony answer header".into(),
            ));
        }
        self.max_response = max_response;
        Ok(self)
    }

    pub fn open_read(&mut self, path: &str) -> Result<u32> {
        let request = Request {
            command: ReadCommand::FileOpen,
            extra: file_open_extra(path)?,
        };
        let answer = self.exchange(request)?;
        decode_handle(&answer.data)
    }

    pub fn close(&mut self, handle: u32) -> Result<()> {
        let request = Request {
            command: ReadCommand::FileClose,
            extra: encode_handle(handle),
        };
        self.exchange(request)?;
        Ok(())
    }

    pub fn size(&mut self, handle: u32) -> Result<u64> {
        let request = Request {
            command: ReadCommand::GetSize,
            extra: encode_handle(handle),
        };
        let answer = self.exchange(request)?;
        decode_integer(&answer.data, "file size")
    }

    pub fn set_position(&mut self, handle: u32, position: u64) -> Result<()> {
        let request = Request {
            command: ReadCommand::SetPosition,
            extra: set_position_extra(handle, position),
        };
        self.exchange(request)?;
        Ok(())
    }

    pub fn read(&mut self, handle: u32, count: usize) -> Result<Vec<u8>> {
        if count == 0 {
            return Err(Error::InvalidArgument("read size must be non-zero".into()));
        }
        if count > self.max_response - 16 {
            return Err(Error::InvalidArgument(format!(
                "read size is {count}, maximum is {}",
                self.max_response - 16
            )));
        }
        let request = Request {
            command: ReadCommand::FileRead,
            extra: file_read_extra(handle, count)?,
        };
        let answer = self.exchange(request)?;
        if answer.data.len() > count {
            return Err(Error::Protocol(format!(
                "device returned {} bytes for a {count}-byte read",
                answer.data.len()
            )));
        }
        Ok(answer.data)
    }

    pub fn copy_path<W: Write>(&mut self, path: &str, output: &mut W) -> Result<u64> {
        let handle = self.open_read(path)?;
        let result = self.copy_open_handle(handle, output);
        let close_result = self.close(handle);
        match (result, close_result) {
            (Ok(bytes), Ok(())) => Ok(bytes),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    fn copy_open_handle<W: Write>(&mut self, handle: u32, output: &mut W) -> Result<u64> {
        let size = self.size(handle)?;
        let chunk_size = self.max_response - 16;
        let mut position = 0u64;
        while position < size {
            self.set_position(handle, position)?;
            let remaining = size - position;
            let count = remaining.min(chunk_size as u64) as usize;
            let bytes = self.read(handle, count)?;
            if bytes.is_empty() {
                return Err(Error::Protocol(format!(
                    "device returned no data at offset {position} of {size}"
                )));
            }
            output.write_all(&bytes)?;
            position += bytes.len() as u64;
        }
        Ok(position)
    }

    fn exchange(&mut self, request: Request) -> Result<Answer> {
        let response_capacity = self.response_capacity(&request)?;
        self.transport.exchange(&request, response_capacity)
    }

    fn response_capacity(&self, request: &Request) -> Result<usize> {
        let data_capacity = match request.command {
            ReadCommand::FileOpen => 4,
            ReadCommand::FileClose | ReadCommand::SetPosition => 0,
            ReadCommand::GetSize | ReadCommand::GetPosition => 8,
            ReadCommand::FileRead => {
                let count = request.extra.get(4..8).ok_or_else(|| {
                    Error::Protocol("file read request is missing its count".into())
                })?;
                u32::from_le_bytes(count.try_into().expect("slice length checked")) as usize
            }
            _ => self.max_response.saturating_sub(ANSWER_HEADER_LEN),
        };
        let response_capacity = ANSWER_HEADER_LEN
            .checked_add(data_capacity)
            .ok_or_else(|| Error::InvalidArgument("response capacity overflows usize".into()))?;
        if response_capacity > self.max_response {
            return Err(Error::InvalidArgument(format!(
                "response capacity is {response_capacity}, maximum is {}",
                self.max_response
            )));
        }
        Ok(response_capacity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ReadCommand;

    struct ScriptedTransport {
        requests: Vec<Request>,
        response_capacities: Vec<usize>,
    }

    impl ScriptedTransport {
        fn answer(data: &[u8]) -> Answer {
            Answer {
                data: data.to_vec(),
            }
        }
    }

    impl PacketTransport for ScriptedTransport {
        fn exchange(&mut self, request: &Request, max_response: usize) -> Result<Answer> {
            self.requests.push(request.clone());
            self.response_capacities.push(max_response);
            let command = request.command as u32;
            match command {
                x if x == ReadCommand::FileOpen as u32 => Ok(Self::answer(&7u32.to_le_bytes())),
                x if x == ReadCommand::GetSize as u32 => Ok(Self::answer(&5u32.to_le_bytes())),
                x if x == ReadCommand::SetPosition as u32 => Ok(Self::answer(&[])),
                x if x == ReadCommand::FileRead as u32 => {
                    let offset = self
                        .requests
                        .iter()
                        .find(|candidate| candidate.command == ReadCommand::SetPosition)
                        .and_then(|candidate| candidate.extra.get(4..12))
                        .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
                        .unwrap_or(0);
                    let data: &[u8] = if offset == 0 { b"hello" } else { &[] };
                    Ok(Self::answer(data))
                }
                x if x == ReadCommand::FileClose as u32 => Ok(Self::answer(&[])),
                _ => Err(Error::Protocol("unexpected test command".into())),
            }
        }
    }

    #[test]
    fn copy_path_uses_only_read_operations_and_closes_handle() {
        let transport = ScriptedTransport {
            requests: Vec::new(),
            response_capacities: Vec::new(),
        };
        let mut client = ReaderClient::new(transport).with_max_response(64).unwrap();
        let mut output = Vec::new();
        let bytes = client
            .copy_path("/Data/tmp/info/model", &mut output)
            .unwrap();
        assert_eq!(bytes, 5);
        assert_eq!(output, b"hello");
        assert!(client
            .transport
            .requests
            .iter()
            .all(|request| request.extra.len() <= 4096));
        assert_eq!(
            client
                .transport
                .requests
                .last()
                .map(|request| request.command as u32),
            Some(ReadCommand::FileClose as u32)
        );
        assert_eq!(
            client.transport.response_capacities,
            vec![20, 24, 16, 21, 16]
        );
        let forbidden = [
            0x13u32, 0x17, 0x1a, 0x1b, 0x1c, 0x30, 0x31, 0x32, 0x300, 0x301, 0x302, 0x303,
        ];
        assert!(client.transport.requests.iter().all(|request| {
            let command = request.command as u32;
            !forbidden.contains(&command)
        }));
    }
}
