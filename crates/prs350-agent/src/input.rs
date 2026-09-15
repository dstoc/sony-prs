use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputEvent {
    Touch { pressed: bool, x: u16, y: u16 },
    Key { code: u8, state: u8 },
}

pub(super) struct SubCpuInput {
    file: std::fs::File,
    pending: Vec<u8>,
}

impl SubCpuInput {
    pub(super) fn open() -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open("/dev/subcpu")?;
        file.write_all(&SUBCPU_SCAN_ON_PACKET)?;
        Ok(Self {
            file,
            pending: Vec::with_capacity(SUBCPU_PACKET_SIZE * 2),
        })
    }

    pub(super) fn next_event(&mut self) -> io::Result<Option<InputEvent>> {
        let mut descriptor = PollFd {
            fd: self.file.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        };
        let result = unsafe { poll(&mut descriptor, 1, 100) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(None);
            }
            return Err(error);
        }
        if result == 0 {
            return Ok(None);
        }
        if descriptor.revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "subcpu input stream closed",
            ));
        }
        if descriptor.revents & POLLIN == 0 {
            return Ok(None);
        }

        let mut bytes = [0u8; 64];
        match self.file.read(&mut bytes) {
            Ok(0) => return Ok(None),
            Ok(length) => self.pending.extend_from_slice(&bytes[..length]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(error) => return Err(error),
        }

        while self.pending.len() >= SUBCPU_PACKET_SIZE {
            if self.pending[0] & 0x80 == 0 {
                self.pending.remove(0);
                continue;
            }
            let packet: [u8; SUBCPU_PACKET_SIZE] = self.pending[..SUBCPU_PACKET_SIZE]
                .try_into()
                .expect("packet length checked");
            self.pending.drain(..SUBCPU_PACKET_SIZE);
            if let Some(event) = decode_subcpu_packet(packet) {
                return Ok(Some(event));
            }
        }
        Ok(None)
    }
}

pub(super) const fn pack_subcpu_packet(raw: [u8; 7]) -> [u8; SUBCPU_PACKET_SIZE] {
    [
        0x80 | (raw[0] >> 1),
        ((raw[0] << 6) | (raw[1] >> 2)) & 0x7f,
        ((raw[1] << 5) | (raw[2] >> 3)) & 0x7f,
        ((raw[2] << 4) | (raw[3] >> 4)) & 0x7f,
        ((raw[3] << 3) | (raw[4] >> 5)) & 0x7f,
        ((raw[4] << 2) | (raw[5] >> 6)) & 0x7f,
        ((raw[5] << 1) | (raw[6] >> 7)) & 0x7f,
        raw[6] & 0x7f,
    ]
}

fn unpack_subcpu_packet(packet: [u8; SUBCPU_PACKET_SIZE]) -> [u8; 7] {
    [
        (packet[0] << 1) | (packet[1] >> 6),
        (packet[1] << 2) | (packet[2] >> 5),
        (packet[2] << 3) | (packet[3] >> 4),
        (packet[3] << 4) | (packet[4] >> 3),
        (packet[4] << 5) | (packet[5] >> 2),
        (packet[5] << 6) | (packet[6] >> 1),
        (packet[6] << 7) | packet[7],
    ]
}

pub(super) fn decode_subcpu_packet(packet: [u8; SUBCPU_PACKET_SIZE]) -> Option<InputEvent> {
    let raw = unpack_subcpu_packet(packet);
    if raw.iter().fold(0u8, |checksum, byte| checksum ^ byte) != 0 {
        return None;
    }

    let category = raw[0] & 0x3f;
    let command = raw[1] & 0x3f;
    match (category, command) {
        (3, 1) => Some(InputEvent::Key {
            code: raw[2],
            state: raw[3],
        }),
        (6, 4) | (6, 5) | (6, 6) => {
            let x = (((raw[2] & 0x0f) as u16) << 8) | raw[3] as u16;
            let y = (((raw[4] & 0x0f) as u16) << 8) | raw[5] as u16;
            let x = interpolate_touch(x, 513, 3588, 100, 500, 599);
            let y = interpolate_touch(y, 3411, 677, 100, 700, 799);
            Some(InputEvent::Touch {
                pressed: command != 5,
                x,
                y,
            })
        }
        _ => None,
    }
}

pub(super) fn interpolate_touch(
    value: u16,
    raw_start: i32,
    raw_end: i32,
    screen_start: i32,
    screen_end: i32,
    screen_max: i32,
) -> u16 {
    let value = i32::from(value);
    let numerator = (value - raw_start) * (screen_end - screen_start);
    let denominator = raw_end - raw_start;
    let mapped = screen_start + numerator / denominator;
    mapped.clamp(0, screen_max) as u16
}
