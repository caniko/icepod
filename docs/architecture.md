# Architecture

`icepod` is an Iced client. `icepodctl` is an operator client. Both connect to `icepod-audio-service` over authenticated, versioned local sockets. The service owns recorder state and files; disconnecting either client has no recording side effect.

`icepod-core` contains the shared protocol, state machine, preallocated realtime block handoff, journal, session model, RF64/BWF writer, and recovery logic. It has no Iced dependency. The CPAL dependency is optional and confined to the service's `hardware` feature.

The deterministic simulator generates a channel-specific sequence where each sample encodes channel and frame identity. Tests therefore detect swaps, duplication, omission, and loss of synchronization exactly.

Phase 1 intentionally has no database, audio graph, plugins, waveform editor, playback engine, resampler, or recorded DSP.
