---
reth-cli-commands: patch
---

Added `snapshot_api_url` field to `DownloadDefaults` so downstream projects can override the snapshot discovery API endpoint. Previously, `discover_manifest_url`, `fetch_snapshot_api_entries`, and `print_snapshot_listing` used a hardcoded `snapshots.reth.rs` URL that bypassed the `DownloadDefaults` override mechanism.
