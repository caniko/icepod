use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use time::{OffsetDateTime, format_description::well_known::Iso8601};
use uuid::Uuid;

use crate::{
    journal::{self, Journal, JournalRecordKind},
    session::{ChannelManifest, Incident, IncidentKind, Marker, SessionManifest, SessionStatus},
    state::RecorderState,
};

const AUDIO_OFFSET: u64 = 690;
const BYTES_PER_SAMPLE: u64 = 4;

#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("at least one channel must be armed")]
    NoChannels,
    #[error("sample rate must be nonzero")]
    InvalidSampleRate,
    #[error("interleaved sample count {samples} is not aligned to {channels} channels")]
    PartialFrame { samples: usize, channels: usize },
    #[error("simulated disk full after {limit} audio bytes")]
    DiskFull { limit: u64 },
    #[error("recorder is not in {expected:?} state")]
    InvalidState { expected: RecorderState },
    #[error("recovery found no safely committed frames")]
    NoCommittedFrames,
}

#[derive(Debug, Clone)]
pub struct RecorderConfig {
    pub sessions_root: PathBuf,
    pub session_name: String,
    pub sample_rate: u32,
    pub channel_labels: Vec<String>,
    pub device_identity: String,
    pub max_audio_bytes: Option<u64>,
}

impl RecorderConfig {
    pub fn estimated_bytes_per_second(&self) -> u64 {
        u64::from(self.sample_rate) * self.channel_labels.len() as u64 * BYTES_PER_SAMPLE
    }

    pub fn estimated_safe_seconds(&self, available_bytes: u64, safety_margin: u64) -> u64 {
        available_bytes.saturating_sub(safety_margin) / self.estimated_bytes_per_second().max(1)
    }
}

pub struct Recorder {
    session_dir: PathBuf,
    files: Vec<File>,
    journal: Journal,
    manifest: SessionManifest,
    max_audio_bytes: Option<u64>,
}

impl Recorder {
    pub fn create(config: RecorderConfig) -> Result<Self, RecorderError> {
        if config.channel_labels.is_empty() {
            return Err(RecorderError::NoChannels);
        }
        if config.sample_rate == 0 {
            return Err(RecorderError::InvalidSampleRate);
        }
        let session_id = Uuid::new_v4();
        let take_id = Uuid::new_v4();
        let created_at = OffsetDateTime::now_utc();
        let timestamp = created_at
            .format(&time::macros::format_description!(
                "[year][month][day]_[hour][minute][second]"
            ))
            .map_err(io::Error::other)?;
        let session_dir = config.sessions_root.join(format!(
            "{timestamp}_{}_{}",
            sanitize(&config.session_name),
            session_id
        ));
        let audio_dir = session_dir.join("audio");
        fs::create_dir_all(&audio_dir)?;
        fs::create_dir_all(session_dir.join("logs"))?;
        fs::create_dir_all(session_dir.join("recovery"))?;

        let channels: Vec<_> = config
            .channel_labels
            .iter()
            .enumerate()
            .map(|(index, label)| ChannelManifest {
                index: index as u16,
                label: label.clone(),
                part_file: format!("take-0001_ch-{:02}_{}.wav.part", index + 1, sanitize(label)),
                final_file: None,
            })
            .collect();
        let mut files = Vec::with_capacity(channels.len());
        for channel in &channels {
            let mut file = OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .open(audio_dir.join(&channel.part_file))?;
            write_rf64_header(
                &mut file,
                config.sample_rate,
                session_id,
                take_id,
                &channel.label,
                &config.device_identity,
                created_at,
            )?;
            files.push(file);
        }

        let mut manifest = SessionManifest {
            format_version: 1,
            session_id,
            take_id,
            name: config.session_name,
            created_at,
            sample_rate: config.sample_rate,
            state: RecorderState::Armed,
            status: SessionStatus::Active,
            frames_written: 0,
            frames_committed: 0,
            channels,
            markers: Vec::new(),
            incidents: Vec::new(),
        };
        manifest.write_atomic(&session_dir.join("session.json"))?;
        let mut journal = Journal::create(&session_dir.join("session.journal"), session_id)?;
        journal.append(JournalRecordKind::SessionCreated, true)?;
        journal.append(JournalRecordKind::DeviceConfigured, true)?;
        journal.append(JournalRecordKind::TakeArmed, true)?;
        manifest.state = manifest
            .state
            .transition(RecorderState::Recording)
            .map_err(io::Error::other)?;
        journal.append(JournalRecordKind::SegmentStarted, true)?;
        manifest.write_atomic(&session_dir.join("session.json"))?;
        Ok(Self {
            session_dir,
            files,
            journal,
            manifest,
            max_audio_bytes: config.max_audio_bytes,
        })
    }

    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    pub fn manifest(&self) -> &SessionManifest {
        &self.manifest
    }

    pub fn write_interleaved(&mut self, samples: &[f32]) -> Result<u64, RecorderError> {
        if self.manifest.state != RecorderState::Recording {
            return Err(RecorderError::InvalidState {
                expected: RecorderState::Recording,
            });
        }
        let channels = self.files.len();
        if !samples.len().is_multiple_of(channels) {
            return Err(RecorderError::PartialFrame {
                samples: samples.len(),
                channels,
            });
        }
        let frames = (samples.len() / channels) as u64;
        let new_frames = self.manifest.frames_written + frames;
        let total_audio_bytes = new_frames * channels as u64 * BYTES_PER_SAMPLE;
        if self
            .max_audio_bytes
            .is_some_and(|limit| total_audio_bytes > limit)
        {
            self.record_incident(
                IncidentKind::DiskCritical,
                None,
                "simulated disk-full boundary",
            )?;
            return Err(RecorderError::DiskFull {
                limit: self.max_audio_bytes.unwrap_or_default(),
            });
        }
        for (channel, file) in self.files.iter_mut().enumerate() {
            let mut bytes = Vec::with_capacity(frames as usize * BYTES_PER_SAMPLE as usize);
            for frame in samples.chunks_exact(channels) {
                bytes.extend_from_slice(&frame[channel].to_le_bytes());
            }
            file.write_all(&bytes)?;
        }
        self.manifest.frames_written = new_frames;
        self.journal.append(
            JournalRecordKind::FramesWritten { frames: new_frames },
            false,
        )?;
        self.commit()?;
        Ok(frames)
    }

    pub fn insert_gap(&mut self, frames: u64) -> Result<(), RecorderError> {
        let channels = self.files.len();
        let start = self.manifest.frames_written;
        let silence = vec![0.0; frames as usize * channels];
        self.write_interleaved(&silence)?;
        self.journal.append(
            JournalRecordKind::GapInserted {
                frame: start,
                frames,
            },
            true,
        )?;
        self.record_incident(
            IncidentKind::GapInserted,
            Some(frames),
            "known sequence gap filled with silence",
        )
    }

    pub fn pause(&mut self) -> Result<(), RecorderError> {
        self.manifest.state = self
            .manifest
            .state
            .transition(RecorderState::Paused)
            .map_err(io::Error::other)?;
        self.journal.append(
            JournalRecordKind::PauseStarted {
                frame: self.manifest.frames_written,
            },
            true,
        )?;
        self.persist_manifest()
    }

    pub fn resume(&mut self) -> Result<(), RecorderError> {
        self.manifest.state = self
            .manifest
            .state
            .transition(RecorderState::Recording)
            .map_err(io::Error::other)?;
        self.journal.append(
            JournalRecordKind::PauseEnded {
                frame: self.manifest.frames_written,
            },
            true,
        )?;
        self.persist_manifest()
    }

    pub fn add_marker(&mut self, request_id: Uuid, note: String) -> Result<Marker, RecorderError> {
        if let Some(marker) = self
            .manifest
            .markers
            .iter()
            .find(|marker| marker.request_id == request_id)
        {
            return Ok(marker.clone());
        }
        let marker = Marker {
            id: Uuid::new_v4(),
            request_id,
            frame: self.manifest.frames_written,
            note,
        };
        self.journal.append(
            JournalRecordKind::MarkerAdded {
                marker_id: marker.id,
                frame: marker.frame,
                note: marker.note.clone(),
            },
            true,
        )?;
        self.manifest.markers.push(marker.clone());
        self.persist_manifest()?;
        Ok(marker)
    }

    pub fn record_incident(
        &mut self,
        kind: IncidentKind,
        missing_frames: Option<u64>,
        detail: impl Into<String>,
    ) -> Result<(), RecorderError> {
        let frame = self.manifest.frames_written;
        let record = match kind {
            IncidentKind::BackendXrun => JournalRecordKind::XrunObserved { frame },
            IncidentKind::QueueOverflow => JournalRecordKind::QueueOverflow {
                frame,
                missing_frames,
            },
            IncidentKind::TimestampDiscontinuity | IncidentKind::UnknownDiscontinuity => {
                JournalRecordKind::TimestampDiscontinuity {
                    frame,
                    missing_frames,
                }
            }
            IncidentKind::GapInserted => JournalRecordKind::GapInserted {
                frame,
                frames: missing_frames.unwrap_or_default(),
            },
            IncidentKind::InputDeviceLost => JournalRecordKind::InputDeviceLost { frame },
            IncidentKind::MonitorDeviceLost => JournalRecordKind::MonitorDeviceLost { frame },
            IncidentKind::DiskWarning => JournalRecordKind::DiskWarning { available_bytes: 0 },
            IncidentKind::DiskCritical | IncidentKind::WriterFailure => {
                JournalRecordKind::DiskCritical { available_bytes: 0 }
            }
        };
        self.journal.append(record, true)?;
        self.manifest.incidents.push(Incident {
            id: Uuid::new_v4(),
            kind,
            frame,
            missing_frames,
            detail: detail.into(),
        });
        self.persist_manifest()
    }

    pub fn finalize(mut self) -> Result<SessionManifest, RecorderError> {
        self.manifest.state = self
            .manifest
            .state
            .transition(RecorderState::Stopping)
            .map_err(io::Error::other)?;
        self.journal
            .append(JournalRecordKind::StopRequested, true)?;
        self.commit()?;
        self.journal.append(
            JournalRecordKind::SegmentStopped {
                frames: self.manifest.frames_committed,
            },
            true,
        )?;
        self.manifest.state = self
            .manifest
            .state
            .transition(RecorderState::Finalizing)
            .map_err(io::Error::other)?;
        self.journal
            .append(JournalRecordKind::FinalizationStarted, true)?;
        for (index, file) in self.files.iter_mut().enumerate() {
            finalize_rf64(file, self.manifest.frames_committed)?;
            file.sync_all()?;
            validate_rf64(
                file,
                self.manifest.sample_rate,
                self.manifest.frames_committed,
            )?;
            let part = self
                .session_dir
                .join("audio")
                .join(&self.manifest.channels[index].part_file);
            let final_name = self.manifest.channels[index]
                .part_file
                .trim_end_matches(".part")
                .to_owned();
            fs::rename(part, self.session_dir.join("audio").join(&final_name))?;
            self.manifest.channels[index].final_file = Some(final_name);
        }
        self.manifest.state = self
            .manifest
            .state
            .transition(RecorderState::Completed)
            .map_err(io::Error::other)?;
        self.manifest.status = SessionStatus::Complete;
        self.journal
            .append(JournalRecordKind::FinalizationCompleted, true)?;
        self.persist_manifest()?;
        fs::write(
            self.session_dir.join("completed.json"),
            serde_json::to_vec_pretty(&self.manifest).map_err(io::Error::other)?,
        )?;
        Ok(self.manifest)
    }

    fn commit(&mut self) -> Result<(), RecorderError> {
        for file in &self.files {
            file.sync_data()?;
        }
        self.manifest.frames_committed = self.manifest.frames_written;
        self.journal.append(
            JournalRecordKind::FramesCommitted {
                frames: self.manifest.frames_committed,
            },
            true,
        )?;
        self.persist_manifest()
    }

    fn persist_manifest(&self) -> Result<(), RecorderError> {
        self.manifest
            .write_atomic(&self.session_dir.join("session.json"))?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct RecoveryOutcome {
    pub session_id: Uuid,
    pub recovered_frames: u64,
    pub files: Vec<PathBuf>,
    pub status: SessionStatus,
}

pub fn recover_session(session_dir: &Path) -> Result<RecoveryOutcome, RecorderError> {
    let manifest_path = session_dir.join("session.json");
    let mut manifest = SessionManifest::load(&manifest_path)?;
    let records = journal::read_valid(&session_dir.join("session.journal"))?;
    let committed = records
        .iter()
        .filter_map(|record| match record.kind {
            JournalRecordKind::FramesCommitted { frames } => Some(frames),
            _ => None,
        })
        .max()
        .unwrap_or_default();
    if committed == 0 {
        return Err(RecorderError::NoCommittedFrames);
    }
    let mut safe_frames = committed;
    for channel in &manifest.channels {
        let length = fs::metadata(session_dir.join("audio").join(&channel.part_file))?.len();
        safe_frames = safe_frames.min(length.saturating_sub(AUDIO_OFFSET) / BYTES_PER_SAMPLE);
    }
    if safe_frames == 0 {
        return Err(RecorderError::NoCommittedFrames);
    }
    let recovery_dir = session_dir.join("recovery");
    fs::create_dir_all(&recovery_dir)?;
    let mut outputs = Vec::with_capacity(manifest.channels.len());
    for channel in &manifest.channels {
        let source_path = session_dir.join("audio").join(&channel.part_file);
        let output_path = recovery_dir
            .join(channel.part_file.trim_end_matches(".wav.part").to_owned() + ".recovered.wav");
        let mut source = File::open(source_path)?;
        source.seek(SeekFrom::Start(AUDIO_OFFSET))?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&output_path)?;
        write_rf64_header(
            &mut output,
            manifest.sample_rate,
            manifest.session_id,
            manifest.take_id,
            &channel.label,
            "recovered",
            manifest.created_at,
        )?;
        io::copy(
            &mut source.take(safe_frames * BYTES_PER_SAMPLE),
            &mut output,
        )?;
        finalize_rf64(&mut output, safe_frames)?;
        output.sync_all()?;
        validate_rf64(&mut output, manifest.sample_rate, safe_frames)?;
        outputs.push(output_path);
    }
    let status = if safe_frames == committed {
        SessionStatus::Recovered
    } else {
        SessionStatus::PartiallyRecovered
    };
    manifest.status = status;
    manifest.state = RecorderState::Completed;
    manifest.frames_written = safe_frames;
    manifest.frames_committed = safe_frames;
    manifest.write_atomic(&manifest_path)?;
    let mut journal = Journal::open(&session_dir.join("session.journal"), manifest.session_id)?;
    journal.append(JournalRecordKind::RecoveryStarted, true)?;
    journal.append(
        JournalRecordKind::RecoveryCompleted {
            frames: safe_frames,
        },
        true,
    )?;
    Ok(RecoveryOutcome {
        session_id: manifest.session_id,
        recovered_frames: safe_frames,
        files: outputs,
        status,
    })
}

fn write_rf64_header(
    file: &mut File,
    sample_rate: u32,
    session_id: Uuid,
    take_id: Uuid,
    channel_label: &str,
    device_identity: &str,
    created_at: OffsetDateTime,
) -> io::Result<()> {
    file.write_all(b"RF64")?;
    file.write_all(&u32::MAX.to_le_bytes())?;
    file.write_all(b"WAVEds64")?;
    file.write_all(&28_u32.to_le_bytes())?;
    file.write_all(&0_u64.to_le_bytes())?;
    file.write_all(&0_u64.to_le_bytes())?;
    file.write_all(&0_u64.to_le_bytes())?;
    file.write_all(&0_u32.to_le_bytes())?;
    file.write_all(b"bext")?;
    file.write_all(&602_u32.to_le_bytes())?;
    let description = format!(
        "Icepod session {session_id}; take {take_id}; channel {channel_label}; device {device_identity}"
    );
    write_fixed(file, description.as_bytes(), 256)?;
    write_fixed(file, b"Icepod", 32)?;
    write_fixed(file, session_id.to_string().as_bytes(), 32)?;
    let timestamp = created_at
        .format(&Iso8601::DEFAULT)
        .map_err(io::Error::other)?;
    write_fixed(
        file,
        timestamp.get(..10).unwrap_or("1970-01-01").as_bytes(),
        10,
    )?;
    write_fixed(
        file,
        timestamp.get(11..19).unwrap_or("00:00:00").as_bytes(),
        8,
    )?;
    file.write_all(&0_u64.to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&[0; 64])?;
    file.write_all(&[0; 10])?;
    file.write_all(&[0; 180])?;
    file.write_all(b"fmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&3_u16.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&(sample_rate * 4).to_le_bytes())?;
    file.write_all(&4_u16.to_le_bytes())?;
    file.write_all(&32_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&u32::MAX.to_le_bytes())?;
    debug_assert_eq!(file.stream_position()?, AUDIO_OFFSET);
    Ok(())
}

fn finalize_rf64(file: &mut File, frames: u64) -> io::Result<()> {
    let data_size = frames * BYTES_PER_SAMPLE;
    file.seek(SeekFrom::Start(20))?;
    file.write_all(&(AUDIO_OFFSET + data_size - 8).to_le_bytes())?;
    file.write_all(&data_size.to_le_bytes())?;
    file.write_all(&frames.to_le_bytes())?;
    file.seek(SeekFrom::End(0))?;
    Ok(())
}

fn validate_rf64(file: &mut File, sample_rate: u32, frames: u64) -> io::Result<()> {
    let expected_length = AUDIO_OFFSET + frames * BYTES_PER_SAMPLE;
    if file.metadata()?.len() != expected_length {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "RF64 length mismatch",
        ));
    }
    let mut header = [0; 48];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;
    if &header[..4] != b"RF64" || &header[8..12] != b"WAVE" || &header[12..16] != b"ds64" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid RF64 header",
        ));
    }
    let data_size = u64::from_le_bytes(header[28..36].try_into().unwrap_or_default());
    let sample_count = u64::from_le_bytes(header[36..44].try_into().unwrap_or_default());
    if data_size != frames * BYTES_PER_SAMPLE || sample_count != frames || sample_rate == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid RF64 ds64 values",
        ));
    }
    file.seek(SeekFrom::End(0))?;
    Ok(())
}

fn write_fixed(file: &mut File, value: &[u8], length: usize) -> io::Result<()> {
    let used = value.len().min(length);
    file.write_all(&value[..used])?;
    file.write_all(&vec![0; length - used])
}

fn sanitize(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "untitled".into()
    } else {
        trimmed.chars().take(64).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(root: &Path, channels: usize) -> RecorderConfig {
        RecorderConfig {
            sessions_root: root.into(),
            session_name: "test session".into(),
            sample_rate: 48_000,
            channel_labels: (0..channels)
                .map(|index| format!("channel-{index}"))
                .collect(),
            device_identity: "simulated".into(),
            max_audio_bytes: None,
        }
    }

    #[test]
    fn exact_multichannel_write_and_finalize() {
        let temporary = tempfile::tempdir().unwrap();
        let mut recorder = Recorder::create(config(temporary.path(), 2)).unwrap();
        recorder
            .write_interleaved(&[0.0, 10.0, 1.0, 11.0, 2.0, 12.0])
            .unwrap();
        let directory = recorder.session_dir().to_owned();
        let manifest = recorder.finalize().unwrap();
        assert_eq!(manifest.frames_committed, 3);
        for (channel, expected) in [[0.0_f32, 1.0, 2.0], [10.0, 11.0, 12.0]].iter().enumerate() {
            let path = directory
                .join("audio")
                .join(manifest.channels[channel].final_file.as_ref().unwrap());
            let mut reader = bwavfile::WaveReader::open(&path).unwrap();
            reader.validate_broadcast_wave().unwrap();
            assert_eq!(reader.frame_length().unwrap(), 3);
            let bytes = fs::read(path).unwrap();
            let actual: Vec<_> = bytes[AUDIO_OFFSET as usize..]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|sample| f32::from_le_bytes(*sample))
                .collect();
            assert_eq!(&actual, expected);
        }
    }

    #[test]
    fn recovery_preserves_parts_and_channel_alignment() {
        let temporary = tempfile::tempdir().unwrap();
        let mut recorder = Recorder::create(config(temporary.path(), 3)).unwrap();
        let samples: Vec<_> = (0..300).map(|index| index as f32).collect();
        recorder.write_interleaved(&samples).unwrap();
        let directory = recorder.session_dir().to_owned();
        drop(recorder);
        let outcome = recover_session(&directory).unwrap();
        assert_eq!(outcome.recovered_frames, 100);
        assert_eq!(outcome.files.len(), 3);
        assert!(directory.join("audio").read_dir().unwrap().all(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "part")
        }));
        assert!(
            outcome
                .files
                .iter()
                .all(|path| fs::metadata(path).unwrap().len() == AUDIO_OFFSET + 400)
        );
    }

    #[test]
    fn disk_limit_fails_before_partial_channel_write() {
        let temporary = tempfile::tempdir().unwrap();
        let mut settings = config(temporary.path(), 2);
        settings.max_audio_bytes = Some(8);
        let mut recorder = Recorder::create(settings).unwrap();
        assert!(matches!(
            recorder.write_interleaved(&[0.0; 4]),
            Err(RecorderError::DiskFull { .. })
        ));
        assert_eq!(recorder.manifest().frames_committed, 0);
    }

    #[test]
    fn duration_estimate_reserves_margin() {
        let settings = config(Path::new("."), 2);
        assert_eq!(settings.estimated_bytes_per_second(), 384_000);
        assert_eq!(settings.estimated_safe_seconds(1_384_000, 1_000_000), 1);
    }

    #[test]
    fn known_gap_inserts_aligned_silence_and_incident() {
        let temporary = tempfile::tempdir().unwrap();
        let mut recorder = Recorder::create(config(temporary.path(), 2)).unwrap();
        recorder.write_interleaved(&[1.0, 2.0]).unwrap();
        recorder.insert_gap(3).unwrap();
        assert_eq!(recorder.manifest().frames_committed, 4);
        assert_eq!(
            recorder.manifest().incidents[0].kind,
            IncidentKind::GapInserted
        );
    }
}
