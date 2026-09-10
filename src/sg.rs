use crate::error::{Error, Result, ScsiError};
use std::fs::{File, OpenOptions};
use std::os::raw::{c_int, c_ulong, c_void};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SG_IO: c_ulong = 0x2285;
const SG_DXFER_NONE: i32 = -1;
const SG_DXFER_TO_DEV: i32 = -2;
const SG_DXFER_FROM_DEV: i32 = -3;
const SG_INFO_OK_MASK: u32 = 0x01;
const INQUIRY: u8 = 0x12;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[repr(C)]
struct SgIoHdr {
    interface_id: c_int,
    dxfer_direction: c_int,
    cmd_len: u8,
    mx_sb_len: u8,
    iovec_count: u16,
    dxfer_len: u32,
    dxferp: *mut c_void,
    cmdp: *mut u8,
    sbp: *mut u8,
    timeout: u32,
    flags: u32,
    pack_id: i32,
    usr_ptr: *mut c_void,
    status: u8,
    masked_status: u8,
    msg_status: u8,
    sb_len_wr: u8,
    host_status: u16,
    driver_status: u16,
    resid: i32,
    duration: u32,
    info: u32,
}

extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDirection {
    None,
    ToDevice,
    FromDevice,
}

impl DataDirection {
    fn as_raw(self) -> i32 {
        match self {
            Self::None => SG_DXFER_NONE,
            Self::ToDevice => SG_DXFER_TO_DEV,
            Self::FromDevice => SG_DXFER_FROM_DEV,
        }
    }
}

#[derive(Debug)]
pub struct SgResponse {
    pub status: u8,
    pub host_status: u16,
    pub driver_status: u16,
    pub bytes_transferred: usize,
    pub sense: Vec<u8>,
}

#[derive(Debug)]
pub struct SgDevice {
    file: File,
    path: PathBuf,
    timeout: Duration,
}

impl SgDevice {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        Ok(Self {
            file,
            path,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub(crate) fn execute(
        &self,
        cdb: &[u8],
        direction: DataDirection,
        data: &mut [u8],
    ) -> Result<SgResponse> {
        if cdb.is_empty() || cdb.len() > 16 {
            return Err(Error::InvalidArgument(
                "SCSI CDB must contain 1 to 16 bytes".into(),
            ));
        }
        if matches!(direction, DataDirection::None) && !data.is_empty() {
            return Err(Error::InvalidArgument(
                "a no-data SCSI command cannot have a data buffer".into(),
            ));
        }
        let dxfer_len = u32::try_from(data.len())
            .map_err(|_| Error::InvalidArgument("SCSI data buffer is too large".into()))?;
        let timeout_ms = self.timeout.as_millis().min(u32::MAX as u128) as u32;
        let mut cdb_buf = [0u8; 16];
        cdb_buf[..cdb.len()].copy_from_slice(cdb);
        let mut sense = [0u8; 64];
        let mut header = SgIoHdr {
            interface_id: b'S' as c_int,
            dxfer_direction: direction.as_raw(),
            cmd_len: cdb.len() as u8,
            mx_sb_len: sense.len() as u8,
            iovec_count: 0,
            dxfer_len,
            dxferp: if data.is_empty() {
                std::ptr::null_mut()
            } else {
                data.as_mut_ptr().cast()
            },
            cmdp: cdb_buf.as_mut_ptr(),
            sbp: sense.as_mut_ptr(),
            timeout: timeout_ms,
            flags: 0,
            pack_id: 0,
            usr_ptr: std::ptr::null_mut(),
            status: 0,
            masked_status: 0,
            msg_status: 0,
            sb_len_wr: 0,
            host_status: 0,
            driver_status: 0,
            resid: 0,
            duration: 0,
            info: 0,
        };

        let result = unsafe { ioctl(self.file.as_raw_fd(), SG_IO, &mut header) };
        if result < 0 {
            return Err(io_error());
        }

        let sense_len = usize::from(header.sb_len_wr).min(sense.len());
        let response = SgResponse {
            status: header.status,
            host_status: header.host_status,
            driver_status: header.driver_status,
            bytes_transferred: transferred_len(data.len(), header.resid),
            sense: sense[..sense_len].to_vec(),
        };
        let failed = response.status != 0
            || response.host_status != 0
            || response.driver_status != 0
            || (header.info & SG_INFO_OK_MASK) != 0;
        if failed {
            return Err(Error::Scsi(ScsiError {
                status: response.status,
                host_status: response.host_status,
                driver_status: response.driver_status,
                sense: response.sense,
            }));
        }
        Ok(response)
    }

    pub fn inquiry(&self) -> Result<Inquiry> {
        let mut data = [0u8; 96];
        let cdb = [INQUIRY, 0, 0, 0, data.len() as u8, 0];
        let response = self.execute(&cdb, DataDirection::FromDevice, &mut data)?;
        let length = response.bytes_transferred.min(data.len());
        if length < 36 {
            return Err(Error::Protocol(format!(
                "SCSI INQUIRY returned {length} bytes, need at least 36"
            )));
        }
        Ok(Inquiry {
            peripheral_type: data[0] & 0x1f,
            vendor: field_string(&data[8..16]),
            product: field_string(&data[16..32]),
            revision: field_string(&data[32..36]),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inquiry {
    pub peripheral_type: u8,
    pub vendor: String,
    pub product: String,
    pub revision: String,
}

fn field_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

fn transferred_len(buffer_len: usize, resid: i32) -> usize {
    if resid <= 0 {
        buffer_len
    } else {
        buffer_len.saturating_sub(resid as usize)
    }
}

fn io_error() -> Error {
    Error::Io(std::io::Error::last_os_error())
}
