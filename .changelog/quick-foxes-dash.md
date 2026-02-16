---
reth-engine-tree: patch
---

Inlined receipt root computation for small blocks (≤50 transactions) to avoid background task overhead. Small blocks now compute the receipt root synchronously instead of spawning a crossbeam channel and tokio task.
