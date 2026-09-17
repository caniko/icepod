use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

use crc32fast::Hasher;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAGIC: &[u8; 8] = b"ICEPODJ\0";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 8 + 2 + 16 + 8 + 2 + 4;
pub const MAX_RECORD_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalRecordKind {
    SessionCreated,
    DeviceConfigured,
    TakeArmed,
    SegmentStarted,
    FramesWritten {
        frames: u64,
    },
    FramesCommitted {
        frames: u64,
    },
    MarkerAdded {
        marker_id: Uuid,
        frame: u64,
        note: String,
    },
    PauseStarted {
        frame: u64,
    },
    PauseEnded {
        frame: u64,
    },
    XrunObserved {
        frame: u64,
    },
    QueueOverflow {
        frame: u64,
        missing_frames: Option<u64>,
    },
    TimestampDiscontinuity {
        frame: u64,
        missing_frames: Option<u64>,
    },
    GapInserted {
        frame: u64,
        frames: u64,
    },
    InputDeviceLost {
        frame: u64,
    },
    MonitorDeviceLost {
        frame: u64,
    },
    DiskWarning {
        available_bytes: u64,
    },
    DiskCritical {
        available_bytes: u64,
    },
    StopRequested,
    SegmentStopped {
        frames: u64,
    },
    FinalizationStarted,
    FinalizationCompleted,
    RecoveryStarted,
    RecoveryCompleted {
        frames: u64,
    },
    SessionFaulted {
        detail: String,
    },
}

impl JournalRecordKind {
    fn code(&self) -> u16 {
        match self {
            Self::SessionCreated => 1,
            Self::DeviceConfigured => 2,
            Self::TakeArmed => 3,
            Self::SegmentStarted => 4,
            Self::FramesWritten { .. } => 5,
            Self::FramesCommitted { .. } => 6,
            Self::MarkerAdded { .. } => 7,
            Self::PauseStarted { .. } => 8,
            Self::PauseEnded { .. } => 9,
            Self::XrunObserved { .. } => 10,
            Self::QueueOverflow { .. } => 11,
            Self::TimestampDiscontinuity { .. } => 12,
            Self::GapInserted { .. } => 13,
            Self::InputDeviceLost { .. } => 14,
            Self::MonitorDeviceLost { .. } => 15,
            Self::DiskWarning { .. } => 16,
            Self::DiskCritical { .. } => 17,
            Self::StopRequested => 18,
            Self::SegmentStopped { .. } => 19,
            Self::FinalizationStarted => 20,
            Self::FinalizationCompleted => 21,
            Self::RecoveryStarted => 22,
            Self::RecoveryCompleted { .. } => 23,
            Self::SessionFaulted { .. } => 24,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRecord {
    pub session_id: Uuid,
    pub sequence: u64,
    pub kind: JournalRecordKind,
}

pub struct Journal {
    file: File,
    session_id: Uuid,
    next_sequence: u64,
}

impl Journal {
    pub fn create(path: &Path, session_id: Uuid) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            file,
            session_id,
            next_sequence: 0,
        })
    }

    pub fn open(path: &Path, session_id: Uuid) -> io::Result<Self> {
        let records = read_valid(path)?;
        if records.iter().any(|record| record.session_id != session_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "journal session mismatch",
            ));
        }
        let next_sequence = records.last().map_or(0, |record| record.sequence + 1);
        let file = OpenOptions::new().append(true).open(path)?;
        Ok(Self {
            file,
            session_id,
            next_sequence,
        })
    }

    pub fn append(&mut self, kind: JournalRecordKind, durable: bool) -> io::Result<u64> {
        let payload = postcard::to_stdvec(&kind).map_err(io::Error::other)?;
        if payload.len() > MAX_RECORD_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "journal record too large",
            ));
        }
        let sequence = self.next_sequence;
        let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len() + 4);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(self.session_id.as_bytes());
        bytes.extend_from_slice(&sequence.to_le_bytes());
        bytes.extend_from_slice(&kind.code().to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&crc32fast::hash(&bytes).to_le_bytes());
        self.file.write_all(&bytes)?;
        if durable {
            self.file.sync_data()?;
        }
        self.next_sequence += 1;
        Ok(sequence)
    }
}

pub fn read_valid(path: &Path) -> io::Result<Vec<JournalRecord>> {
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(parse_valid(&bytes))
}

pub fn parse_valid(bytes: &[u8]) -> Vec<JournalRecord> {
    let mut records = Vec::new();
    let mut offset = 0;
    while bytes.len().saturating_sub(offset) >= HEADER_LEN + 4 {
        let header = &bytes[offset..offset + HEADER_LEN];
        if &header[..8] != MAGIC || u16::from_le_bytes([header[8], header[9]]) != VERSION {
            break;
        }
        let mut session = [0; 16];
        session.copy_from_slice(&header[10..26]);
        let sequence = u64::from_le_bytes(header[26..34].try_into().unwrap_or_default());
        let code = u16::from_le_bytes(header[34..36].try_into().unwrap_or_default());
        let payload_len =
            u32::from_le_bytes(header[36..40].try_into().unwrap_or_default()) as usize;
        if payload_len > MAX_RECORD_SIZE {
            break;
        }
        let record_len = match HEADER_LEN
            .checked_add(payload_len)
            .and_then(|length| length.checked_add(4))
        {
            Some(length) if length <= bytes.len() - offset => length,
            _ => break,
        };
        let record_bytes = &bytes[offset..offset + record_len];
        let checksum_offset = record_len - 4;
        let stored = u32::from_le_bytes(
            record_bytes[checksum_offset..]
                .try_into()
                .unwrap_or_default(),
        );
        let mut hasher = Hasher::new();
        hasher.update(&record_bytes[..checksum_offset]);
        if hasher.finalize() != stored {
            break;
        }
        let kind = match postcard::from_bytes::<JournalRecordKind>(
            &record_bytes[HEADER_LEN..checksum_offset],
        ) {
            Ok(kind) if kind.code() == code => kind,
            _ => break,
        };
        let session_id = Uuid::from_bytes(session);
        if records.last().is_some_and(|previous: &JournalRecord| {
            previous.session_id != session_id || previous.sequence + 1 != sequence
        }) {
            break;
        }
        records.push(JournalRecord {
            session_id,
            sequence,
            kind,
        });
        offset += record_len;
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_keeps_records_before_truncation_or_corruption() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("session.journal");
        let mut journal = Journal::create(&path, Uuid::nil()).unwrap();
        journal
            .append(JournalRecordKind::SessionCreated, true)
            .unwrap();
        journal
            .append(JournalRecordKind::FramesCommitted { frames: 64 }, true)
            .unwrap();
        let complete = std::fs::read(&path).unwrap();
        for length in 0..complete.len() {
            let parsed = parse_valid(&complete[..length]);
            assert!(parsed.len() <= 2);
            if parsed.len() == 2 {
                assert_eq!(
                    parsed[1].kind,
                    JournalRecordKind::FramesCommitted { frames: 64 }
                );
            }
        }
        let mut corrupt = complete;
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert_eq!(parse_valid(&corrupt).len(), 1);
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        for length in 0..512 {
            let bytes: Vec<_> = (0..length).map(|index| (index * 31) as u8).collect();
            let _ = parse_valid(&bytes);
        }
    }
}
