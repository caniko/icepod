# ADR 0002: RF64/BWF and Journal

Status: accepted.

Icepod writes RF64 from the first sample with a fixed BEXT-bearing provisional header. Audio synchronization precedes a checksummed `FramesCommitted` journal record. `bwavfile 2.0.1` is the independent final parser because hard termination cannot rely on a writer destructor. Recovery reconstructs separate files and preserves interrupted originals.
