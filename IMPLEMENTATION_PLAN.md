# Issue #20482: Add Configuration Validation for RocksDB Flags at Startup

## Problem

Reth uses `StorageSettings` to control where data is stored (MDBX vs RocksDB vs static files). These flags are:
- `receipts_in_static_files`
- `transaction_senders_in_static_files`
- `storages_history_in_rocksdb`
- `transaction_hash_numbers_in_rocksdb`
- `account_history_in_rocksdb`

**The bug**: Users can change these flags via CLI after initial sync, but the node silently loads the *persisted* settings from the database, ignoring CLI flags. This creates confusion:

1. User syncs with `--static-files.receipts=false` (receipts in MDBX)
2. User restarts with `--static-files.receipts=true` (expecting receipts in static files)
3. Node loads persisted settings from DB → still uses MDBX
4. User thinks receipts are in static files, but they're not

**Worse scenario**: If CLI flags were honored on restart, old data would be in one location, new data in another → data corruption.

## Design Philosophy

Following John Ousterhout's *A Philosophy of Software Design*, we chose to **"Define Errors Out of Existence"** rather than adding complex error handling in the storage layer.

### Why Not Add Validation in `ProviderFactory::new()`?

The original approach was to:
1. Add `expected_settings` parameter to `ProviderFactory::new()`
2. Add `StorageSettingsMismatch` error variant
3. Return error if settings don't match

**Problems with this approach:**
- Adds complexity to a core type (`ProviderFactory`)
- Forces all callers to handle a new error type
- The "error" has no meaningful recovery - it's really a configuration mistake

### Better Approach: Validate at CLI Layer

Instead, we:
1. Keep `ProviderFactory::new()` simple - it just loads settings from DB
2. Validate settings mismatch in CLI code where user intent is clear
3. Return a clear, actionable error message using `eyre::eyre!`

## Solution Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              CLI LAYER                                       │
│                                                                              │
│   1. Create ProviderFactory (loads settings from DB)                         │
│   2. Compare CLI settings vs stored settings                                 │
│   3. If mismatch → return eyre::eyre! with clear message                     │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                           STORAGE LAYER                                      │
│                                                                              │
│   ProviderFactory::new() - UNCHANGED                                         │
│   • Loads settings from DB                                                   │
│   • Falls back to legacy defaults if none exist                              │
│   • No validation, no new parameters                                         │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Implementation

### Part 1: CLI Validation (`crates/cli/commands/src/common.rs`)

In `create_provider_factory()`, after creating the factory:

```rust
// Error if CLI settings differ from stored settings
let cli_settings = self.static_files.to_settings();
if let Some(stored_settings) = factory.storage_settings()? {
    if stored_settings != cli_settings {
        return Err(eyre::eyre!(
            "Storage settings mismatch!\n\n\
             Stored in DB: {stored_settings:?}\n\
             From CLI:     {cli_settings:?}\n\n\
             Storage flags cannot be changed after genesis.\n\
             Either remove the conflicting CLI flags, or delete the database and re-sync."
        ));
    }
}
```

### Part 2: Node Builder Validation (`crates/node/builder/src/launch/common.rs`)

Same pattern in `create_provider_factory()`:

```rust
// Error if CLI settings differ from stored settings
let cli_settings = self.node_config().static_files.to_settings();
if let Some(stored_settings) = factory.storage_settings()? {
    if stored_settings != cli_settings {
        return Err(eyre::eyre!(
            "Storage settings mismatch!\n\n\
             Stored in DB: {stored_settings:?}\n\
             From CLI:     {cli_settings:?}\n\n\
             Storage flags cannot be changed after genesis.\n\
             Either remove the conflicting CLI flags, or delete the database and re-sync."
        ));
    }
}
```

## Files Modified

| File | Change |
|------|--------|
| `crates/cli/commands/src/common.rs` | Add settings mismatch check |
| `crates/node/builder/src/launch/common.rs` | Add settings mismatch check |

## Files NOT Modified (Key Insight!)

| File | Why Not |
|------|---------|
| `crates/storage/errors/src/provider.rs` | No new error type needed |
| `crates/storage/provider/src/providers/database/mod.rs` | `ProviderFactory::new()` stays simple |
| `crates/storage/provider/src/providers/database/builder.rs` | No new builder methods |

## Benefits of This Approach

1. **Simpler core types** - `ProviderFactory` doesn't need to know about CLI settings
2. **No new error variants** - Uses existing `eyre` for CLI errors
3. **Clear error messages** - Actionable guidance at the user-facing layer
4. **Follows single responsibility** - Storage layer stores, CLI layer validates user input

## Testing

The validation is straightforward and tested by:
1. Starting a node with settings A
2. Restarting with settings B
3. Observing the clear error message

No unit tests needed in the storage layer since no logic was added there.

## PR

- Title: `feat(cli): validate storage settings match persisted settings at startup`
- Closes #20482