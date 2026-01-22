---
title: Add configuration validation for RocksDB flags at startup
labels:
    - A-db
    - C-enhancement
assignees:
    - yongkangc
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 20384
blocked_by:
    - 20393
synced_at: 2026-01-21T11:32:16.015217Z
info:
    author: yongkangc
    created_at: 2025-12-18T09:26:00Z
    updated_at: 2026-01-21T11:02:57Z
---

Requires: https://github.com/paradigmxyz/reth/issues/20392 and #20393 

Configuration validation:
- Warn or error if user tries to change RocksDB flags after initial sync
- The flags should only be set at genesis initialization

Prevent users from changing RocksDB flags after initial sync, which would cause data inconsistency between MDBX and RocksDB.

Contributes to RETH-112

**Current State**
- Settings persisted at genesis via `write_storage_settings()` in `init.rs`
- Settings loaded on startup in `ProviderFactory::new()`
- **No validation** that settings haven't changed after chain progressed

**Required Changes**
Add validation in `create_provider_factory()` or `ProviderFactory::new()`:

```rust
// If chain has progressed past genesis
let tip = provider.last_block_number()?;
if tip > genesis_block && persisted_settings != current_settings {
    return Err(ConfigMismatchError {
        stored: persisted_settings,
        current: current_settings,
        message: "RocksDB flags cannot be changed after initial sync"
    });
}
```

Closes RETH-175
