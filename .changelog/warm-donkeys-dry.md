---
reth-engine-local: minor
reth-node-builder: minor
---

Added trigger-based `MiningMode` variant that allows blocks to be built on-demand via custom streams, and exposed `with_mining_mode` method on `DebugNodeLauncherFuture` to override default mining configuration.
