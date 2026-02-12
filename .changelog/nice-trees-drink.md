---
reth-network-api: minor
reth-network-types: minor
reth-network: minor
reth-node-core: minor
reth: minor
---

Added optional ENR fork ID enforcement to filter out peers from incompatible networks during peer discovery, controlled by the `--enforce-enr-fork-id` CLI flag.
