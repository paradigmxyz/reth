---
reth-engine-tree: patch
---

Downgraded per-transaction prewarm span from `debug_span!` to `trace_span!` to reduce noise in debug-level logging.
