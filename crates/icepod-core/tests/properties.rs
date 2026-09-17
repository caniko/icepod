use icepod_core::{CaptureCallback, RecorderState, journal::parse_valid};
use proptest::prelude::*;

proptest! {
    #[test]
    fn journal_parser_accepts_arbitrary_bytes_without_panicking(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let _ = parse_valid(&bytes);
    }

    #[test]
    fn arbitrary_callback_boundaries_preserve_samples(
        samples in proptest::collection::vec(-1.0_f32..1.0, 1..2048),
        callback_sizes in proptest::collection::vec(1_usize..128, 1..64),
    ) {
        let (mut callback, mut worker) = CaptureCallback::new(64, 64, 1);
        let mut offset = 0;
        let mut output = Vec::new();
        for size in callback_sizes.into_iter().cycle() {
            if offset == samples.len() {
                break;
            }
            let end = (offset + size).min(samples.len());
            callback.capture(&samples[offset..end], None);
            offset = end;
            while let Some(block) = worker.pop() {
                output.extend_from_slice(block.samples());
                worker.recycle(block).unwrap();
            }
        }
        callback.flush_partial();
        while let Some(block) = worker.pop() {
            output.extend_from_slice(block.samples());
            worker.recycle(block).unwrap();
        }
        prop_assert_eq!(output, samples);
    }

    #[test]
    fn arbitrary_state_targets_never_bypass_transition_validation(targets in proptest::collection::vec(0_u8..10, 0..100)) {
        let states = [
            RecorderState::Idle,
            RecorderState::Configured,
            RecorderState::Armed,
            RecorderState::Recording,
            RecorderState::Paused,
            RecorderState::Stopping,
            RecorderState::Finalizing,
            RecorderState::Completed,
            RecorderState::Faulted,
            RecorderState::Recovering,
        ];
        let mut state = RecorderState::Idle;
        for target in targets {
            if let Ok(next) = state.transition(states[target as usize]) {
                state = next;
            }
        }
        prop_assert!(states.contains(&state));
    }
}
