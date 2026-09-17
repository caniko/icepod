# ADR 0004: Realtime Memory Model

Status: accepted.

Use a fixed pool of boxed audio blocks and paired `rtrb` SPSC queues. Callback exhaustion drops and accounts for input instead of blocking. Heap activity is allowed during initialization and writer work, never callback execution. This is tested with a process-wide allocation guard.
