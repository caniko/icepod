use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::state::RecorderState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionStatus {
    Active,
    Complete,
    Interrupted,
    Recovered,
    PartiallyRecovered,
    NeedsAttention,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub id: Uuid,
    pub request_id: Uuid,
    pub frame: u64,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IncidentKind {
    BackendXrun,
    QueueOverflow,
    TimestampDiscontinuity,
    UnknownDiscontinuity,
    GapInserted,
    InputDeviceLost,
    MonitorDeviceLost,
    DiskWarning,
    DiskCritical,
    WriterFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incident {
    pub id: Uuid,
    pub kind: IncidentKind,
    pub frame: u64,
    pub missing_frames: Option<u64>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelManifest {
    pub index: u16,
    pub label: String,
    pub part_file: String,
    pub final_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionManifest {
    pub format_version: u16,
    pub session_id: Uuid,
    pub take_id: Uuid,
    pub name: String,
    pub created_at: OffsetDateTime,
    pub sample_rate: u32,
    pub state: RecorderState,
    pub status: SessionStatus,
    pub frames_written: u64,
    pub frames_committed: u64,
    pub channels: Vec<ChannelManifest>,
    pub markers: Vec<Marker>,
    pub incidents: Vec<Incident>,
}

impl SessionManifest {
    pub fn write_atomic(&self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("manifest has no parent"))?;
        fs::create_dir_all(parent)?;
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        fs::write(&temporary, bytes)?;
        let file = fs::OpenOptions::new().read(true).open(&temporary)?;
        file.sync_all()?;
        fs::rename(temporary, path)?;
        sync_directory(parent)
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)
    }
}

fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_manifest_round_trip() {
        let temporary = tempfile::tempdir().unwrap();
        let manifest = SessionManifest {
            format_version: 1,
            session_id: Uuid::nil(),
            take_id: Uuid::nil(),
            name: "test".into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            sample_rate: 48_000,
            state: RecorderState::Idle,
            status: SessionStatus::Active,
            frames_written: 0,
            frames_committed: 0,
            channels: Vec::new(),
            markers: Vec::new(),
            incidents: Vec::new(),
        };
        let path = temporary.path().join("session.json");
        manifest.write_atomic(&path).unwrap();
        assert_eq!(SessionManifest::load(&path).unwrap().name, "test");
    }
}
