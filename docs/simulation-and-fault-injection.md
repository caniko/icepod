# Simulation and Fault Injection

The simulator emits deterministic interleaved f32 samples. Channel `c`, frame `f` is `c * 1_000_000 + f`. Callback size is independent of recorder block size.

Use `icepodctl simulate FRAMES --callback-frames N` while recording. `icepodctl inject` supports `xrun`, `queue-overflow`, `timestamp-jump`, `input-lost`, `monitor-lost`, and `disk-critical`; `--missing-frames N` makes a timestamp jump insert synchronized silence.

`icepodctl soak` writes a JSON report. Accelerated mode uses a 100 Hz logical sample rate to represent four hours without creating an 11 GB fixture. `--realtime` uses 48 kHz and one-second pacing.
