# IPC Protocol

Icepod uses `interprocess` local sockets: filesystem Unix sockets where supported and local named namespaces otherwise. There is no TCP listener.

Each frame has eight magic bytes, major/minor versions, a little-endian 32-bit payload length, and a postcard payload. Frames larger than 1 MiB are rejected before allocation. Commands carry UUID request IDs; the service caches 1024 results so duplicate transport requests do not repeat markers or transitions.

The service creates a per-user mode-0700 runtime directory and mode-0600 random token file. Every request authenticates with that token, which tracing never records. Stale filesystem sockets are reclaimed with `try_overwrite`.

Current connections perform one request/response and reconnect cheaply. Explicit Hello/capability and heartbeat event streams, peer-credential validation, and bounded per-client event queues remain open.
