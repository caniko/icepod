# ADR 0001: Audio Service Process

Status: accepted.

The audio service is a separately packaged process and owns authoritative state. GUI lifetime must not control recording lifetime. Clients reconnect and request snapshots; only explicit `Stop` finalizes a take.
