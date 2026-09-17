#![forbid(unsafe_code)]

pub mod ipc;
pub mod journal;
pub mod protocol;
pub mod realtime;
pub mod recorder;
pub mod session;
pub mod state;

pub use journal::{Journal, JournalRecord, JournalRecordKind};
pub use protocol::{
    Command, CommandResult, DeviceInfo, FaultKind, Request, Response, StateSnapshot,
};
pub use realtime::{AudioBlock, CaptureCallback, CaptureWorker, Meter, SimulatedAudio};
pub use recorder::{Recorder, RecorderConfig, RecoveryOutcome, recover_session};
pub use session::{Incident, IncidentKind, Marker, SessionManifest, SessionStatus};
pub use state::{RecorderState, StateError};
