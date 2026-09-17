# Crash Recovery

Audio is synchronized for every deterministic writer batch. A `FramesCommitted` record is appended and synchronized only after every channel file reaches that common durable frontier. The current conservative durability window is one writer batch; physical CPAL batching is not connected yet.

Recovery reads journal records until the first incomplete, oversized, invalid, out-of-sequence, or checksum-failing record. It intersects the last committed frontier with every physical channel length and rounds down to complete float samples. It then reconstructs independent RF64/BWF files under `recovery/`, synchronizes and validates them, and updates the session classification. Original `.wav.part` files are never modified.

Failure behavior:

- GUI crash: service and recording continue.
- Service crash, abrupt kill, or power loss: audio through the last synchronized frontier is recoverable; later bytes are not claimed.
- Input loss: durable incident and fault state exist; automatic clean segment finalization is not yet wired to CPAL.
- Monitor loss: durable incident; deterministic recording continues.
- Queue overflow or xrun: distinct durable incidents; known gaps can be filled with aligned silence.
- Disk full or writer failure: explicit fault path preserves prior commits; proactive real-device stop remains open.
- Partial journal record: ignored with all preceding records retained.
- Partial audio sample/frame: discarded by the common complete-frame calculation.
- Invalid provisional header: reconstructed from manifest metadata.
- Failed final rename: `.part` remains; recovery can produce a separate output.
- Failed recovery validation: original remains authoritative; the caller receives an error.
