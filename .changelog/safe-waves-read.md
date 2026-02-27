---
reth-network-types: minor
reth-network: minor
reth-node-core: patch
---

Added `PersistedPeerInfo` struct to persist richer peer metadata (kind, fork ID, reputation) to disk. Updated `PeersConfig::with_basic_nodes_from_file` to support both the new `PersistedPeerInfo` format and the legacy `Vec<NodeRecord>` format with automatic conversion, and updated `write_peers_to_file` to exclude backed-off and banned peers.
