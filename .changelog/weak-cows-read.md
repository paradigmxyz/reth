---
reth-node-metrics: minor
reth-rpc-api: minor
reth-rpc: minor
reth-rpc-builder: patch
reth-rpc-eth-api: patch
reth-node-builder: patch
reth-cli-commands: minor
reth-e2e-test-utils: patch
---

Added `reth_forkSchedule` RPC method returning the full hardfork schedule with activation conditions and active status. Extended `ChainSpecInfo` with `from_hardforks` constructor to expose per-fork metrics (block number, timestamp, or TTD) via Prometheus using the generic `Hardforks` trait, supporting both Ethereum and custom chain forks.
