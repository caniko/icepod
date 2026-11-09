# Icepod vision

Icepod is a podcast-first desktop recorder. Its defining choice is that the
audio service and the raw multichannel recording live independently of the Iced
GUI: the service owns recorder state and files, and both the GUI and the
`icepodctl` operator client are projections that connect over authenticated
local sockets. Disconnecting either client has no recording side effect.

Phase 1 is a complete deterministic recording and recovery slice — RF64/BWF
output, a journal, take browsing, and recovery — but not yet a release-ready
physical recorder.

## Problem

Recording a conversation should not depend on a window staying open or a GUI
staying responsive. Tools built around an editor or a single stereo input lose
takes when the app crashes, when a device disappears, or when the interface is
closed mid-session. Podcast recording also starts multichannel: host and guest
feeds stay separate until a mix decision, rather than being baked together at
capture time.

## Principles

- **The service is authoritative.** `icepod-audio-service` owns state and files;
  the GUI and CLI reconnect and retrieve a snapshot instead of holding the
  session.
- **Determinism is testable.** The simulator encodes channel and frame identity
  in every sample, so tests detect swaps, duplication, omission, and loss of
  synchronization exactly.
- **Realtime work is preallocated.** `icepod-core` owns the protocol, state
  machine, preallocated block handoff, journal, session model, RF64/BWF writer,
  and recovery logic, with no Iced dependency and CPAL confined to the service's
  `hardware` feature.
- **Recovery never overwrites.** Validated recovered outputs go to the session's
  `recovery/` directory, and existing `.wav.part` files are left alone.

## Scope

- authoritative audio service with local authenticated IPC
- Iced setup, recording, and take-browser projection
- `icepodctl` operator commands: configure, start, simulate, pause, resume,
  mark, stop, list, inspect, recover
- synchronized RF64/BWF multichannel writer and journal
- deterministic simulator and fault injection
- CPAL device enumeration behind the `hardware` feature

## Non-goals

Phase 1 intentionally has no database, audio graph, plugins, waveform editor,
playback engine, resampler, or recorded DSP.

## Direction

`PHASE1_STATUS.md` lists the release blockers: connecting selected CPAL input
streams to the preallocated callback handoff, the software monitoring ring and
output callback, bounded meter and health streaming, segment finalization and
resume after device loss, stopping before physical disk exhaustion, validated
marker chunks, and explicit capability negotiation and peer-credential checks.
