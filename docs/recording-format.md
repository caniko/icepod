# Recording Format

Each armed channel is a synchronized mono 32-bit IEEE-float RF64 Broadcast Wave file. Files begin as `.wav.part` and contain `RF64`, `ds64`, `bext`, `fmt `, and `data` chunks. BEXT identifies Icepod, session, take, channel, device, and UTC start.

The data offset is fixed at 690 bytes. RF64 is used from the first frame, so no 4 GB format transition is needed. During finalization Icepod writes the 64-bit RIFF size, data size, and sample count into `ds64`, synchronizes the file, validates length and metadata, validates with `bwavfile`, and only then renames to `.wav`.

Markers remain authoritative in `session.json` and the journal. Cue-chunk mirroring is not implemented yet.
