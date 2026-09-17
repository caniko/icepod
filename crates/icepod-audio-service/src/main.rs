#![forbid(unsafe_code)]

use std::{
    collections::{HashMap, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::Parser;
use directories::{BaseDirs, ProjectDirs};
use icepod_core::session::IncidentKind;
use icepod_core::{
    Command, CommandResult, DeviceInfo, FaultKind, Recorder, RecorderConfig, RecorderState,
    Request, Response, SessionManifest, StateSnapshot, ipc, protocol::ProtocolError,
    recover_session,
};
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions,
    tokio::{Listener, Stream, prelude::*},
};
use miette::{IntoDiagnostic, Result as MietteResult};
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use uuid::Uuid;

const MAX_CACHED_REQUESTS: usize = 1024;

#[derive(Parser)]
#[command(name = "icepod-audio-service")]
struct Args {
    #[arg(long)]
    simulate: bool,
    #[arg(long)]
    runtime_dir: Option<PathBuf>,
    #[arg(long)]
    sessions_dir: Option<PathBuf>,
}

#[derive(Clone)]
struct PendingConfig {
    session_name: String,
    sample_rate: u32,
    channel_labels: Vec<String>,
}

struct Service {
    token: String,
    sessions_root: PathBuf,
    state: RecorderState,
    pending: Option<PendingConfig>,
    recorder: Option<Recorder>,
    cached: HashMap<Uuid, Response>,
    cache_order: VecDeque<Uuid>,
}

impl Service {
    fn execute(&mut self, request: Request) -> Response {
        if request.token != self.token {
            return Response {
                request_id: request.request_id,
                result: Err(ProtocolError::Authentication),
            };
        }
        if let Some(response) = self.cached.get(&request.request_id) {
            return response.clone();
        }
        let result = self.execute_command(request.request_id, request.command);
        let response = Response {
            request_id: request.request_id,
            result,
        };
        self.cached.insert(request.request_id, response.clone());
        self.cache_order.push_back(request.request_id);
        if self.cache_order.len() > MAX_CACHED_REQUESTS
            && let Some(oldest) = self.cache_order.pop_front()
        {
            self.cached.remove(&oldest);
        }
        response
    }

    fn execute_command(
        &mut self,
        request_id: Uuid,
        command: Command,
    ) -> std::result::Result<CommandResult, ProtocolError> {
        match command {
            Command::Status => Ok(CommandResult::Snapshot(self.snapshot())),
            Command::List => Ok(CommandResult::Sessions(list_sessions(&self.sessions_root)?)),
            Command::Devices => Ok(CommandResult::Devices(available_devices()?)),
            Command::Configure {
                session_name,
                sample_rate,
                channel_labels,
            } => {
                if channel_labels.is_empty() || sample_rate == 0 {
                    return Err(ProtocolError::InvalidCommand(
                        "sample rate and armed channels are required".into(),
                    ));
                }
                if !matches!(
                    self.state,
                    RecorderState::Idle
                        | RecorderState::Completed
                        | RecorderState::Configured
                        | RecorderState::Armed
                ) {
                    return Err(ProtocolError::InvalidCommand(format!(
                        "cannot configure while {:?}",
                        self.state
                    )));
                }
                self.pending = Some(PendingConfig {
                    session_name,
                    sample_rate,
                    channel_labels,
                });
                self.state = RecorderState::Armed;
                Ok(CommandResult::Accepted(self.snapshot()))
            }
            Command::Start => {
                if self.state == RecorderState::Recording {
                    return Ok(CommandResult::Accepted(self.snapshot()));
                }
                if self.state != RecorderState::Armed {
                    return Err(ProtocolError::InvalidCommand(format!(
                        "cannot start while {:?}",
                        self.state
                    )));
                }
                let pending = self
                    .pending
                    .clone()
                    .ok_or_else(|| ProtocolError::Service("armed configuration missing".into()))?;
                let recorder = Recorder::create(RecorderConfig {
                    sessions_root: self.sessions_root.clone(),
                    session_name: pending.session_name,
                    sample_rate: pending.sample_rate,
                    channel_labels: pending.channel_labels,
                    device_identity: "deterministic-simulator".into(),
                    max_audio_bytes: None,
                })
                .map_err(service_error)?;
                self.state = RecorderState::Recording;
                self.recorder = Some(recorder);
                Ok(CommandResult::Accepted(self.snapshot()))
            }
            Command::Pause => {
                let recorder = self.recorder.as_mut().ok_or_else(not_recording)?;
                recorder.pause().map_err(service_error)?;
                self.state = RecorderState::Paused;
                Ok(CommandResult::Accepted(self.snapshot()))
            }
            Command::Resume => {
                let recorder = self.recorder.as_mut().ok_or_else(not_recording)?;
                recorder.resume().map_err(service_error)?;
                self.state = RecorderState::Recording;
                Ok(CommandResult::Accepted(self.snapshot()))
            }
            Command::Mark { note } => {
                let recorder = self.recorder.as_mut().ok_or_else(not_recording)?;
                recorder
                    .add_marker(request_id, note)
                    .map_err(service_error)?;
                Ok(CommandResult::Accepted(self.snapshot()))
            }
            Command::Stop => {
                if self.state == RecorderState::Completed {
                    return Ok(CommandResult::Accepted(self.snapshot()));
                }
                let recorder = self.recorder.take().ok_or_else(not_recording)?;
                self.state = RecorderState::Finalizing;
                match recorder.finalize() {
                    Ok(_) => self.state = RecorderState::Completed,
                    Err(error) => {
                        self.state = RecorderState::Faulted;
                        return Err(service_error(error));
                    }
                }
                Ok(CommandResult::Accepted(self.snapshot()))
            }
            Command::Simulate {
                frames,
                callback_frames,
            } => {
                if callback_frames == 0 {
                    return Err(ProtocolError::InvalidCommand(
                        "callback frame count must be nonzero".into(),
                    ));
                }
                let recorder = self.recorder.as_mut().ok_or_else(not_recording)?;
                let channels = recorder.manifest().channels.len();
                let mut next_frame = recorder.manifest().frames_written;
                let target_frame = next_frame.saturating_add(frames);
                while next_frame < target_frame {
                    let count = callback_frames.min((target_frame - next_frame) as usize);
                    let mut samples = Vec::with_capacity(count * channels);
                    for frame in 0..count as u64 {
                        for channel in 0..channels {
                            samples
                                .push(channel as f32 * 1_000_000.0 + (next_frame + frame) as f32);
                        }
                    }
                    recorder
                        .write_interleaved(&samples)
                        .map_err(service_error)?;
                    next_frame += count as u64;
                }
                Ok(CommandResult::Accepted(self.snapshot()))
            }
            Command::InjectFault { fault } => {
                let recorder = self.recorder.as_mut().ok_or_else(not_recording)?;
                match fault {
                    FaultKind::BackendXrun => recorder.record_incident(
                        IncidentKind::BackendXrun,
                        None,
                        "injected backend xrun",
                    ),
                    FaultKind::QueueOverflow { missing_frames } => recorder.record_incident(
                        IncidentKind::QueueOverflow,
                        missing_frames,
                        "injected block-pool exhaustion",
                    ),
                    FaultKind::TimestampJump {
                        missing_frames: Some(frames),
                    } => recorder.insert_gap(frames),
                    FaultKind::TimestampJump {
                        missing_frames: None,
                    } => recorder.record_incident(
                        IncidentKind::UnknownDiscontinuity,
                        None,
                        "injected timestamp jump",
                    ),
                    FaultKind::InputDeviceLost => recorder.record_incident(
                        IncidentKind::InputDeviceLost,
                        None,
                        "injected input-device loss",
                    ),
                    FaultKind::MonitorDeviceLost => recorder.record_incident(
                        IncidentKind::MonitorDeviceLost,
                        None,
                        "injected output-device loss",
                    ),
                    FaultKind::DiskCritical => recorder.record_incident(
                        IncidentKind::DiskCritical,
                        None,
                        "injected disk-critical threshold",
                    ),
                }
                .map_err(service_error)?;
                if matches!(fault, FaultKind::InputDeviceLost | FaultKind::DiskCritical) {
                    self.state = RecorderState::Faulted;
                }
                Ok(CommandResult::Accepted(self.snapshot()))
            }
            Command::Recover { session, all } => {
                let sessions = list_sessions(&self.sessions_root)?;
                let selected: Vec<_> = sessions
                    .into_iter()
                    .filter(|manifest| all || Some(manifest.session_id) == session)
                    .collect();
                if selected.is_empty() {
                    return Err(ProtocolError::InvalidCommand(
                        "no matching interrupted session".into(),
                    ));
                }
                for manifest in selected {
                    let directory = find_session_dir(&self.sessions_root, manifest.session_id)?;
                    recover_session(&directory).map_err(service_error)?;
                }
                Ok(CommandResult::Sessions(list_sessions(&self.sessions_root)?))
            }
            Command::Inspect { session } => {
                let directory = find_session_dir(&self.sessions_root, session)?;
                let manifest = SessionManifest::load(&directory.join("session.json"))
                    .map_err(service_error)?;
                Ok(CommandResult::Session(manifest))
            }
        }
    }

    fn snapshot(&self) -> StateSnapshot {
        let manifest = self.recorder.as_ref().map(Recorder::manifest);
        StateSnapshot {
            state: self.state,
            session_id: manifest.map(|value| value.session_id),
            frames_written: manifest.map_or(0, |value| value.frames_written),
            frames_committed: manifest.map_or(0, |value| value.frames_committed),
            active_channels: manifest.map_or_else(
                || {
                    self.pending
                        .as_ref()
                        .map_or(0, |value| value.channel_labels.len())
                },
                |value| value.channels.len(),
            ),
            markers: manifest.map_or(0, |value| value.markers.len()),
            incidents: manifest.map_or(0, |value| value.incidents.len()),
            available_bytes: fs2::available_space(&self.sessions_root).ok(),
        }
    }
}

#[tokio::main]
async fn main() -> MietteResult<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();
    let args = Args::parse();
    let runtime_dir = args.runtime_dir.unwrap_or_else(default_runtime_dir);
    let sessions_root = args.sessions_dir.unwrap_or_else(default_sessions_dir);
    secure_directory(&runtime_dir).into_diagnostic()?;
    fs::create_dir_all(&sessions_root).into_diagnostic()?;
    let token = Uuid::new_v4().simple().to_string();
    write_token(&runtime_dir.join(ipc::TOKEN_BASENAME), &token).into_diagnostic()?;
    let listener = create_listener(&runtime_dir).into_diagnostic()?;
    info!(simulate = args.simulate, runtime = %runtime_dir.display(), sessions = %sessions_root.display(), "Icepod audio service ready");
    let service = Arc::new(Mutex::new(Service {
        token,
        sessions_root,
        state: RecorderState::Idle,
        pending: None,
        recorder: None,
        cached: HashMap::new(),
        cache_order: VecDeque::new(),
    }));
    loop {
        match listener.accept().await {
            Ok(stream) => {
                let service = Arc::clone(&service);
                tokio::spawn(async move {
                    if let Err(error) = handle_client(stream, service).await {
                        warn!(%error, "client request failed");
                    }
                });
            }
            Err(error) => error!(%error, "local socket accept failed"),
        }
    }
}

async fn handle_client(stream: Stream, service: Arc<Mutex<Service>>) -> io::Result<()> {
    let mut receiver = &stream;
    let request: Request = ipc::read_async(&mut receiver).await?;
    let response = service.lock().await.execute(request);
    let mut sender = &stream;
    ipc::write_async(&mut sender, &response).await
}

fn create_listener(runtime_dir: &Path) -> io::Result<Listener> {
    let options = ListenerOptions::new().try_overwrite(true);
    if GenericFilePath::is_supported() {
        let path = ipc::socket_path(runtime_dir);
        options
            .name(path.as_path().to_fs_name::<GenericFilePath>()?)
            .create_tokio()
    } else {
        options
            .name("icepod-service".to_ns_name::<GenericNamespaced>()?)
            .create_tokio()
    }
}

fn default_runtime_dir() -> PathBuf {
    BaseDirs::new()
        .and_then(|dirs| dirs.runtime_dir().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
        .join("icepod")
}

fn default_sessions_dir() -> PathBuf {
    ProjectDirs::from("org", "caniko", "Icepod").map_or_else(
        || PathBuf::from("sessions"),
        |dirs| dirs.data_local_dir().join("sessions"),
    )
}

fn secure_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_token(path: &Path, token: &str) -> io::Result<()> {
    fs::write(path, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn list_sessions(root: &Path) -> std::result::Result<Vec<SessionManifest>, ProtocolError> {
    let mut sessions = Vec::new();
    for entry in fs::read_dir(root).map_err(service_error)? {
        let path = entry.map_err(service_error)?.path().join("session.json");
        if path.is_file() {
            match SessionManifest::load(&path) {
                Ok(manifest) => sessions.push(manifest),
                Err(error) => {
                    warn!(path = %path.display(), %error, "ignoring unreadable session manifest")
                }
            }
        }
    }
    sessions.sort_by_key(|manifest| manifest.created_at);
    sessions.reverse();
    Ok(sessions)
}

fn find_session_dir(root: &Path, session_id: Uuid) -> std::result::Result<PathBuf, ProtocolError> {
    for entry in fs::read_dir(root).map_err(service_error)? {
        let directory = entry.map_err(service_error)?.path();
        let path = directory.join("session.json");
        if path.is_file()
            && SessionManifest::load(&path).is_ok_and(|manifest| manifest.session_id == session_id)
        {
            return Ok(directory);
        }
    }
    Err(ProtocolError::InvalidCommand(format!(
        "unknown session {session_id}"
    )))
}

fn service_error(error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::Service(error.to_string())
}

fn not_recording() -> ProtocolError {
    ProtocolError::InvalidCommand("no active recording".into())
}

#[cfg(feature = "hardware")]
fn available_devices() -> std::result::Result<Vec<DeviceInfo>, ProtocolError> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let mut result = Vec::new();
    for host_id in cpal::available_hosts() {
        let host = cpal::host_from_id(host_id).map_err(service_error)?;
        for device in host.input_devices().map_err(service_error)? {
            let name = device.to_string();
            for config in device.supported_input_configs().map_err(service_error)? {
                result.push(DeviceInfo {
                    host: host_id.name().into(),
                    name: name.clone(),
                    input_channels: config.channels(),
                    minimum_sample_rate: config.min_sample_rate(),
                    maximum_sample_rate: config.max_sample_rate(),
                    sample_format: config.sample_format().to_string(),
                });
            }
        }
    }
    Ok(result)
}

#[cfg(not(feature = "hardware"))]
fn available_devices() -> std::result::Result<Vec<DeviceInfo>, ProtocolError> {
    Ok(vec![DeviceInfo {
        host: "simulated".into(),
        name: "Deterministic multichannel generator".into(),
        input_channels: 32,
        minimum_sample_rate: 8_000,
        maximum_sample_rate: 384_000,
        sample_format: "f32".into(),
    }])
}
