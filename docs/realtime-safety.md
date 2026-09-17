# Realtime Safety

`CaptureCallback` receives a preallocated `AudioBlock` from an `rtrb` SPSC free queue, copies samples into fixed storage, and pushes full blocks to a second bounded SPSC queue. Blocks carry sequence, stream-relative frame, and optional timestamp values. Arbitrary callback sizes are split or combined without resizing.

The callback does not perform filesystem access, IPC, logging, locking, formatting, device queries, or waits. Empty-pool and full-queue paths increment atomics and drop input instead of blocking. `realtime_alloc.rs` installs `assert_no_alloc::AllocDisabler` and fails on post-initialization callback allocation or deallocation. A timing check compares 256-frame callback work against its 48 kHz buffer period.

Meter calculation is outside the callback. CPAL stream construction is not yet connected to this pipeline; the release gate remains open until that wiring and hardware tests exist.
