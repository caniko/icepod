# Platform Support

The protocol and local socket implementation compile against Unix-domain sockets and Windows named local sockets through `interprocess`. Iced uses its wgpu renderer.

The `hardware` feature enables CPAL 0.18.2 input host/device/configuration enumeration. Linux requires ALSA development metadata at compile time; the Nix shell supplies it. PipeWire/Pulse normally appear through CPAL's Linux host stack. Windows uses WASAPI and macOS uses CoreAudio through CPAL.

Only Linux default-feature compilation and deterministic runtime tests were executed for this phase. Windows/macOS CI jobs are defined but not locally executed. Physical stream capture and monitoring are not implemented yet.
