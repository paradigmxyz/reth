---
reth-rpc-api: minor
reth-rpc-builder: patch
reth-rpc: minor
---

Added `subscribeFinalizedChainNotifications` RPC endpoint that buffers committed chain notifications and emits them once a new finalized block is received.
