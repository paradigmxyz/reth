# v2 Storage Mode — Missing Test Coverage

Audit of `remove-plain-state-gaps` branch. Items below are missing tests identified during review.
v1 = plain slot keys (legacy), v2 = keccak256-hashed slot keys.

## HIGH Priority

### 1. `storage_by_hashed_key` — zero direct tests
- **What**: `StateProvider::storage_by_hashed_key()` is implemented in `LatestStateProviderRef`, `HistoricalStateProviderRef`, and `MemoryOverlayStateProviderRef` but never tested directly.
- **Risk**: Called from `ConsistentProvider::populate_bundle_state()` in v2 mode. If the hashed lookup is wrong, `get_state()` returns incorrect storage values during reorgs.
- **Files**: `crates/storage/provider/src/providers/state/latest.rs`, `crates/storage/provider/src/providers/state/historical.rs`, `crates/chain-state/src/memory_overlay.rs`
- **Suggested test**: Write v2-mode unit tests that insert into `HashedStorages`, then call `storage_by_hashed_key()` and verify the value. Test both latest and historical providers. Verify that passing a plain (unhashed) key does NOT return the value (confirms no accidental plain-state fallback).

### 2. Engine tree live sync not tested in v2 mode
- **What**: Engine tree tests (`crates/engine/tree/src/tree/tests.rs`, `crates/engine/tree/tests/`) never configure `StorageSettings::v2()`. The full `save_blocks` → `write_state` → `write_state_reverts` → static file path is untested in v2.
- **Risk**: Changeset key format corruption during CL-driven block persistence would go undetected.
- **Suggested test**: Add a variant of existing engine tree e2e tests that sets `StorageSettings::v2()` on the provider factory before running.

### 3. `populate_bundle_state_hashed` — no unit test with pre-hashed keys
- **What**: `DatabaseProvider::populate_bundle_state_hashed()` uses `old_storage.key.as_b256()` to look up `HashedStorages`. No test verifies this works with `ChangesetEntry` containing `StorageSlotKey::Hashed` keys.
- **Risk**: Could silently misread `HashedStorages` during reorg if `as_b256()` returns wrong format.
- **File**: `crates/storage/provider/src/providers/database/provider.rs`
- **Suggested test**: Write a unit test that populates `HashedStorages` and `StorageChangeSets` (static file) with hashed keys, calls `get_state()` which triggers `populate_bundle_state_hashed`, and verifies the `BundleState` has correct present values.

### 4. `write_state_reverts` roundtrip not tested in v2
- **What**: `test_write_and_remove_state_roundtrip_legacy` tests v1 only. No v2 equivalent exists.
- **Risk**: `write_state_reverts` hashes keys via `to_changeset_key(true)` before writing. `remove_state_above` then reads them back. If the key format doesn't roundtrip, state removal is silently wrong.
- **File**: `crates/storage/provider/src/providers/database/provider.rs`
- **Suggested test**: Clone `test_write_and_remove_state_roundtrip_legacy`, set `StorageSettings::v2()`, verify that write → remove → verify cycle works with hashed changeset keys.

## MEDIUM Priority

### 5. `get_storage_before_block` full dispatch chain untested in v2
- **What**: The static file `get_storage_before_block` is tested at the manager level, but the full chain (historical lookup → `InChangeset` → `get_storage_before_block` → static file with hashed keys) is not tested end-to-end.
- **File**: `crates/storage/provider/src/providers/state/historical.rs`
- **Suggested test**: Set up v2 provider, write blocks with storage changes to static files, then query historical storage at various block numbers and verify correct values.

### 6. `assert_changesets_queryable` doesn't verify key format
- **What**: The pipeline test helper checks changesets are non-empty and come from the right source, but never asserts keys are hashed in v2 / plain in v1.
- **Risk**: A bug that writes plain keys to v2 static files would pass silently.
- **File**: `crates/stages/stages/tests/pipeline.rs`
- **Suggested fix**: Add assertions that verify at least one changeset key differs from its keccak256 (for v1, key != keccak256(key)) or equals its expected hashed value (for v2).

### 7. `unwind_storage_history_indices` not directly tested in v2
- **What**: `unwind_storage_history_indices` uses `storage.key.as_b256()` for `StoragesHistory` lookups. In v2 this feeds hashed keys. No unit test covers this path.
- **File**: `crates/storage/provider/src/providers/database/provider.rs`
- **Suggested test**: Write history indices with hashed keys, call unwind, verify indices are correctly removed.
