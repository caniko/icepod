use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::{session::SessionManifest, state::RecorderState};

pub const PROTOCOL_MAGIC: [u8; 8] = *b"ICEPODP\0";
pub const PROTOCOL_MAJOR: u16 = 1;
pub const PROTOCOL_MINOR: u16 = 0;
pub const MAX_FRAME_LENGTH: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    Status,
    List,
    Devices,
    Configure {
        session_name: String,
        sample_rate: u32,
        channel_labels: Vec<String>,
    },
    Start,
    Pause,
    Resume,
    Mark {
        note: String,
    },
    Stop,
    Recover {
        session: Option<Uuid>,
        all: bool,
    },
    Inspect {
        session: Uuid,
    },
    Simulate {
        frames: u64,
        callback_frames: usize,
    },
    InjectFault {
        fault: FaultKind,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum FaultKind {
    BackendXrun,
    QueueOverflow { missing_frames: Option<u64> },
    TimestampJump { missing_frames: Option<u64> },
    InputDeviceLost,
    MonitorDeviceLost,
    DiskCritical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub request_id: Uuid,
    pub token: String,
    pub command: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub state: RecorderState,
    pub session_id: Option<Uuid>,
    pub frames_written: u64,
    pub frames_committed: u64,
    pub active_channels: usize,
    pub markers: usize,
    pub incidents: usize,
    pub available_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub host: String,
    pub name: String,
    pub input_channels: u16,
    pub minimum_sample_rate: u32,
    pub maximum_sample_rate: u32,
    pub sample_format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandResult {
    Snapshot(StateSnapshot),
    Devices(Vec<DeviceInfo>),
    Sessions(Vec<SessionManifest>),
    Session(SessionManifest),
    Accepted(StateSnapshot),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub request_id: Uuid,
    pub result: Result<CommandResult, ProtocolError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ProtocolError {
    #[error("authentication failed")]
    Authentication,
    #[error("incompatible protocol version {major}.{minor}")]
    IncompatibleVersion { major: u16, minor: u16 },
    #[error("invalid command: {0}")]
    InvalidCommand(String),
    #[error("service error: {0}")]
    Service(String),
}

pub fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> io::Result<()> {
    writer.write_all(&encode_frame(value)?)
}

pub fn encode_frame<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    let payload = postcard::to_stdvec(value).map_err(io::Error::other)?;
    if payload.len() > MAX_FRAME_LENGTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "IPC frame too large",
        ));
    }
    let mut frame = Vec::with_capacity(16 + payload.len());
    frame.extend_from_slice(&PROTOCOL_MAGIC);
    frame.extend_from_slice(&PROTOCOL_MAJOR.to_le_bytes());
    frame.extend_from_slice(&PROTOCOL_MINOR.to_le_bytes());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<T> {
    let mut header = [0; 16];
    reader.read_exact(&mut header)?;
    if header[..8] != PROTOCOL_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid IPC magic",
        ));
    }
    let major = u16::from_le_bytes([header[8], header[9]]);
    let minor = u16::from_le_bytes([header[10], header[11]]);
    if major != PROTOCOL_MAJOR || minor > PROTOCOL_MINOR {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            ProtocolError::IncompatibleVersion { major, minor },
        ));
    }
    let length = u32::from_le_bytes(header[12..16].try_into().unwrap_or_default()) as usize;
    if length > MAX_FRAME_LENGTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "IPC frame too large",
        ));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    decode_payload(&payload)
}

pub fn decode_payload<T: DeserializeOwned>(payload: &[u8]) -> io::Result<T> {
    postcard::from_bytes(payload).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_round_trip_and_limits() {
        let request = Request {
            request_id: Uuid::nil(),
            token: "secret".into(),
            command: Command::Status,
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request).unwrap();
        let decoded: Request = read_frame(&mut bytes.as_slice()).unwrap();
        assert!(matches!(decoded.command, Command::Status));

        let mut oversized = Vec::from(PROTOCOL_MAGIC);
        oversized.extend_from_slice(&PROTOCOL_MAJOR.to_le_bytes());
        oversized.extend_from_slice(&PROTOCOL_MINOR.to_le_bytes());
        oversized.extend_from_slice(&((MAX_FRAME_LENGTH + 1) as u32).to_le_bytes());
        assert_eq!(
            read_frame::<Request>(&mut oversized.as_slice())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
