---
reth-tasks: patch
---

Added panic handler to `WorkerPool::from_builder` that logs panics via `tracing::error` instead of aborting the process.
