use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RecorderState {
    #[default]
    Idle,
    Configured,
    Armed,
    Recording,
    Paused,
    Stopping,
    Finalizing,
    Completed,
    Faulted,
    Recovering,
}

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[error("invalid recorder transition from {from:?} to {to:?}")]
pub struct StateError {
    pub from: RecorderState,
    pub to: RecorderState,
}

impl RecorderState {
    pub fn transition(self, to: Self) -> Result<Self, StateError> {
        let valid = matches!(
            (self, to),
            (Self::Idle, Self::Configured)
                | (Self::Configured, Self::Armed | Self::Idle)
                | (Self::Armed, Self::Recording | Self::Configured)
                | (
                    Self::Recording,
                    Self::Paused | Self::Stopping | Self::Faulted
                )
                | (
                    Self::Paused,
                    Self::Recording | Self::Stopping | Self::Faulted
                )
                | (Self::Stopping, Self::Finalizing | Self::Faulted)
                | (Self::Finalizing, Self::Completed | Self::Faulted)
                | (Self::Faulted, Self::Recovering)
                | (Self::Recovering, Self::Completed | Self::Faulted)
                | (Self::Completed, Self::Configured | Self::Recovering)
        );
        valid.then_some(to).ok_or(StateError { from: self, to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_and_invalid_transitions_are_centralized() {
        assert_eq!(
            RecorderState::Armed.transition(RecorderState::Recording),
            Ok(RecorderState::Recording)
        );
        assert!(
            RecorderState::Idle
                .transition(RecorderState::Recording)
                .is_err()
        );
        assert!(
            RecorderState::Recording
                .transition(RecorderState::Completed)
                .is_err()
        );
    }
}
