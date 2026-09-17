# Phase 1 Status

Icepod has a working deterministic recording and recovery vertical slice, but it is not ready for release as a physical podcast recorder. The authoritative service, local IPC, CLI, GUI projection, synchronized RF64/BWF writer, journal, recovery path, and simulator are implemented. CPAL currently enumerates devices only; physical capture and software monitoring are not connected.

## Verified on Linux

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` in `nix develop`
- `cargo nextest run --workspace --all-features`: 21 passed
- `cargo test --doc --workspace`
- `cargo doc --workspace --no-deps`
- `cargo deny check`
- `cargo audit` completed with documented transitive warnings
- `nix flake check --show-trace`
- Accelerated four-hour, four-channel soak: 1,440,000 committed frames, synchronized files, zero reported discontinuities
- Live IPC lifecycle: configure, start, simulate, pause, resume, mark, stop, list, and inspect
- CPAL/ALSA device and supported-input-configuration enumeration

Coverage is measured, not used as a release claim. The current all-feature run reports 49.60% total line coverage because process entry points are exercised as child processes and are not merged into the report. Core line coverage ranges from 75.93% for IPC to 100% for state transitions; the requested 90% target is not yet met for every critical module.

## Release Blockers

- Connect selected CPAL input streams to the preallocated callback handoff and writer.
- Add the bounded software-monitoring ring and CPAL output callback.
- Stream meters and service health to clients at a bounded update rate.
- Finalize the current segment and support resume after input-device loss.
- Stop safely before physical disk exhaustion instead of relying only on write errors and estimates.
- Write and independently validate marker cue/label chunks.
- Add explicit capability negotiation, heartbeat events, and peer-credential checks where supported.
- Complete the four-hour wall-clock soak and representative Linux, Windows, and macOS hardware tests.
- Bring critical IPC, journal, recovery, and state-transition coverage to at least 90%.

The detailed gate list is in [`docs/phase-1-release-checklist.md`](docs/phase-1-release-checklist.md).
