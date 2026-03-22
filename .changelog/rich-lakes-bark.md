---
reth-provider: patch
---

Fixed sender pruning during block reorg to skip when sender_recovery is fully pruned, preventing a fatal crash when no sender data exists in static files.
