# Icepod

Icepod is a podcast-first desktop recorder that keeps the audio service and raw multichannel recording alive independently of the Iced GUI.

Phase 1 currently provides a complete deterministic recording and recovery path, authenticated local IPC, `icepodctl`, an Iced setup/recording/take-browser projection, RF64/BWF output, and CPAL device enumeration behind the `hardware` feature. See [PHASE1_STATUS.md](PHASE1_STATUS.md) for exact release-gate status; physical CPAL capture and software monitoring are not yet release-complete.

## Build

```sh
cargo build --workspace
cargo build -p icepod-audio-service --features hardware
```

On Linux, the hardware build requires ALSA development files. `nix develop` supplies native audio and GUI dependencies.

## Run with deterministic audio

Start the authoritative service:

```sh
cargo run -p icepod-audio-service -- --simulate
```

In another terminal:

```sh
cargo run -p icepodctl -- configure --name demo --channels host,guest
cargo run -p icepodctl -- start
cargo run -p icepodctl -- simulate 48000 --callback-frames 257
cargo run -p icepodctl -- mark "chapter one"
cargo run -p icepodctl -- status
cargo run -p icepodctl -- stop
```

Launch the GUI with `cargo run -p icepod -- --simulate`. Closing it does not issue `Stop`; reconnecting retrieves the service snapshot.

## Recovery

```sh
cargo run -p icepodctl -- list
cargo run -p icepodctl -- inspect <session-uuid>
cargo run -p icepodctl -- recover <session-uuid>
cargo run -p icepodctl -- recover --all
```

Recovery never overwrites `.wav.part` files. Validated outputs are placed under the session's `recovery/` directory.

## Checks

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo test --doc --workspace
env CARGO_PROFILE_DEV_CODEGEN_BACKEND=llvm cargo llvm-cov nextest --workspace --all-features
cargo deny check
cargo audit
cargo doc --workspace --no-deps
```

Hardware feature gate in `nix develop`:

```sh
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Accelerated logical and real wall-clock soaks require a running simulated service:

```sh
cargo run -p icepodctl -- soak --hours 4 --channels 4 --report accelerated-soak.json
cargo run -p icepodctl -- soak --hours 4 --channels 4 --realtime --report realtime-soak.json
```
