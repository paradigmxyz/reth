---
reth-tasks: patch
---

Added panic handler to all rayon thread pools that logs panics via `tracing::error` instead of aborting the process.
