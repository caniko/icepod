# ADR 0003: Local IPC

Status: accepted.

Use `interprocess` local sockets and postcard framing. A local TCP port would expand exposure and stale-port behavior without a product requirement. Protocol frames are versioned, bounded, authenticated, and request-ID based.
