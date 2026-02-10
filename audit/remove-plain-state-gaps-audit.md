# Audit: `remove-plain-state-gaps` vs `origin/main`

**Date:** 2026-02-10
**Branch:** `remove-plain-state-gaps`
**Scope:** 51 files changed, ~3500 insertions, ~500 deletions across 30 commits
**Invariant under test:** When `use_hashed_state=true` (v2), `PlainStorageState`/`PlainAccountState` are not used. `HashedStorages`/`HashedAccounts` are the source of truth. `StorageChangeSets` store plain addresses but keccak256-hashed slots.

---

## Summary

**No critical bugs found.** All production code paths correctly enforce the v2 invariant. Three latent risks identified (not currently reachable bugs). Suggestions for hardening included below.

---

## Verified Paths

### Database Provider (`providers/database/provider.rs`)

| Function | Lines | Status | Notes |
|----------|-------|--------|-------|
| `write_state_reverts` | 2433–2525 | ✅ | Keys from `PlainStateReverts` are REVM plain U256 slots → `keccak256` when v2 is correct |
| `write_state_changes` | 2528–2594 | ✅ | Gated: skips `PlainAccountState`/`PlainStorageState` writes when v2 |
| `populate_bundle_state_hashed` | 1258–1322 | ✅ | Reads v2 changeset keys (already hashed), seeks `HashedStorages` directly — no double-hash |
| `remove_state_above` | ~2690 | ✅ | Branches to `populate_bundle_state_hashed` + hashed cursors in v2 |
| `take_state_above` | ~2857 | ✅ | Same branching as `remove_state_above` |
| `unwind_storage_hashing` | 3114–3149 | ✅ | Uses `maybe_hash_key(key, use_hashed_state)` — passthrough in v2, keccak in v1 |
| `changed_storages_and_blocks_with_range` | 2285–2320 | ✅ | Reads raw changeset keys (hashed in v2) for history indices — consistent with `storage_history_lookup` |
| `update_history_indices` | 3331–3344 | ✅ | Delegates to `changed_storages_and_blocks_with_range` |
| `save_blocks` | ~677 | ✅ | Calls `update_history_indices` which reads from changesets (already hashed in v2) |
| `append_blocks_with_state` | 3564–3664 | ✅ | Hashes REVM bundle revert keys (plain U256) for history; uses pre-computed transitions |
| `basic_account` | 1384–1393 | ✅ | `HashedAccounts` in v2, `PlainAccountState` in v1 |
| `basic_accounts` | 1405–1427 | ✅ | Same branching |
| `insert_storage_for_hashing` | 3159–3200 | ⚠️ | Unconditionally hashes — see Latent Risks #1 |

### ConsistentProvider (`providers/consistent.rs`)

| Function | Lines | Status | Notes |
|----------|-------|--------|-------|
| `populate_bundle_state` | 224–290 | ✅ | Uses `storage_by_hashed_key` in v2 to avoid double-hash |
| `storage_changeset` | 1311–1362 | ✅ | In-memory: hashes REVM plain keys when v2; DB: delegates to provider |
| `get_storage_before_block` | 1365–1412 | ✅ | Same pattern |
| `storage_changesets_range` | 1414–1477 | ✅ | Same pattern |

### Historical State Provider (`providers/state/historical.rs`)

| Function | Lines | Status | Notes |
|----------|-------|--------|-------|
| `storage` | 489–500 | ✅ | Hashes storage key before lookup in v2 |
| `storage_by_hashed_key` | 502–511 | ✅ | Passes pre-hashed key directly to `storage_by_lookup_key` |
| `storage_by_lookup_key` | 178–219 | ✅ | `InPlainState` branch reads `HashedStorages` with hashed key in v2 |
| `basic_account` | 306–328 | ✅ | `InPlainState` reads `HashedAccounts` in v2 |
| `storage_history_lookup` | 153–174 | ✅ | Delegates to `EitherReader` with the lookup key as-is |

### Latest State Provider (`providers/state/latest.rs`)

| Function | Lines | Status | Notes |
|----------|-------|--------|-------|
| `basic_account` | 56–66 | ✅ | `HashedAccounts` in v2 |
| `storage` | 178–197 | ✅ | `HashedStorages` with keccak-hashed keys in v2 |
| `storage_by_hashed_key` | 199–209 | ✅ | Passthrough to `hashed_storage_lookup` |

### Memory Overlay (`chain-state/src/memory_overlay.rs`)

| Function | Lines | Status | Notes |
|----------|-------|--------|-------|
| `storage_by_hashed_key` | 227–245 | ✅ | Queries `trie_input.state.storages` (keccak-keyed) then delegates to historical |

### Trie (`trie/db/`)

| Function | File | Status | Notes |
|----------|------|--------|-------|
| `from_reverts` | state.rs:294 | ✅ | Uses `maybe_hash_key` for storage slots from changesets |
| `from_reverts_auto` | state.rs:270 | ✅ | Reads `use_hashed_state` from provider settings |
| `incremental_root_calculator` | state.rs:162 | ✅ | Passes `use_hashed_state` to `load_prefix_sets_with_provider` |
| `load_prefix_sets_with_provider` | prefix_set.rs:27 | ✅ | Uses `maybe_hash_key` for storage keys |
| `hashed_storage_from_reverts_with_provider` | storage.rs:34 | ✅ | Uses `maybe_hash_key` |

### Stages

| Stage | Status | Notes |
|-------|--------|-------|
| Execution | ✅ | Writes hashed state when v2 enabled |
| Storage Hashing (execute) | ✅ | No-op when v2 |
| Storage Hashing (unwind) | ✅ | No-op when v2 |
| Account Hashing (execute) | ✅ | No-op when v2 |
| Account Hashing (unwind) | ✅ | No-op when v2 |
| Merkle | ✅ | Uses `StorageSettingsCache` for `incremental_root_calculator` |

### CLI / Other

| Component | Status | Notes |
|-----------|--------|-------|
| Stage drop command | ✅ | Clears `HashedAccounts`/`HashedStorages` instead of plain tables in v2 |
| `maybe_hash_key` | ✅ | Correct: passthrough when v2 (key already hashed), keccak when v1 |
| `delegate_provider_impls!` macro | ✅ | Includes `storage_by_hashed_key` delegation |

---

## Latent Risks

These are not currently reachable bugs but could become issues if future changes relax the guards.

### 1. `insert_storage_for_hashing` unconditionally hashes (MEDIUM)

**Location:** `crates/storage/provider/src/providers/database/provider.rs:3159–3200`

`insert_storage_for_hashing` applies `keccak256` to all storage keys unconditionally. If ever called with v2 changeset keys (already hashed), it would double-hash and corrupt `HashedStorages`.

**Why safe today:** All production callers are gated:
- Storage hashing stage → no-op in v2
- Genesis init (`db-common/init.rs`) → always plain keys from alloc
- Test code only (`test_utils`, `trie/parallel`, `engine/tree` benchmarks)

**Recommendation:** Add a `debug_assert!(!use_hashed_state)` or a doc comment warning that this function expects plain (unhashed) storage keys.

### 2. `maybe_hash_key` naming is counterintuitive (LOW)

**Location:** `crates/trie/common/src/key.rs:23–29`

The function name suggests "maybe apply a hash" but the boolean controls the *opposite*: `true` = passthrough (no hash), `false` = hash. This encodes "changeset key is already hashed in v2" semantics, but a future contributor could easily invert the logic.

**Recommendation:** Consider renaming to `changeset_key_to_hashed(key, already_hashed)` or adding a clear doc comment explaining the direction. Alternatively, introduce newtypes (`PlainSlotKey` / `HashedSlotKey`) for compile-time safety.

### 3. `storage_by_hashed_key` default panics (LOW)

**Location:** `crates/storage/storage-api/src/state.rs:59–65`

The default implementation uses `unreachable!()`. If a provider that doesn't override this method is used in v2 mode, it panics at runtime.

**Why safe today:** All real providers (`LatestStateProviderRef`, `HistoricalStateProviderRef`, `MemoryOverlayStateProviderRef`, `LatestStateProvider` via macro delegation) override this. `NoopProvider` and `MockEthProvider` inherit the default but are never used in production v2 mode.

**Recommendation:** Consider returning `Err(ProviderError::UnsupportedProvider)` instead of panicking, or add a `#[cfg(debug_assertions)]` guard.

---

## Suggestions

### 1. Guard `insert_storage_for_hashing` against v2 misuse

Add a debug assertion at the top of the function:

```rust
fn insert_storage_for_hashing(...) -> ProviderResult<...> {
    debug_assert!(
        !self.cached_storage_settings().use_hashed_state,
        "insert_storage_for_hashing expects plain (unhashed) storage keys; \
         in v2 mode, HashedStorages is written directly by execution"
    );
    // ... existing code
}
```

### 2. Consider type-level key safety

The most dangerous class of bug in this branch is double-hashing (or missing a hash). Both are silent data corruption. Long-term, consider introducing:

```rust
struct PlainSlotKey(B256);
struct HashedSlotKey(B256);

impl PlainSlotKey {
    fn to_changeset_key(self, v2: bool) -> B256 {
        if v2 { keccak256(self.0) } else { self.0 }
    }
}
```

This makes double-hashing a compile error rather than a runtime bug.

### 3. Add an integration test for the full write→unwind→verify cycle in v2

The existing tests cover individual components well, but a single test that:
1. Writes state for multiple blocks with v2 enabled
2. Verifies `HashedStorages` contains expected values
3. Unwinds back to genesis
4. Verifies `HashedStorages` is restored to pre-state
5. Verifies `PlainStorageState` remains empty throughout

...would catch any cross-component inconsistency.

### 4. Document the key encoding contract

Add a module-level doc comment (e.g., in `storage-api/src/state.rs` or a new `docs/v2-storage-mode.md`) that formally states:

- `StorageChangeSets`: address = plain, storage_key = `keccak256(slot)` in v2 / plain in v1
- `StoragesHistory`: address = plain, storage_key = matches changeset encoding
- `HashedStorages`: address = `keccak256(address)`, storage_key = `keccak256(slot)` (always)
- `HashedAccounts`: key = `keccak256(address)` (always)

This prevents future contributors from making incorrect assumptions.

---

## Test Coverage

| Test | Mode | What it covers |
|------|------|----------------|
| `test_latest_storage_hashed_state` | v2 | LatestStateProviderRef reads from HashedStorages |
| `test_latest_storage_legacy` | v1 | LatestStateProviderRef reads from PlainStorageState |
| `test_latest_storage_hashed_state_returns_none_for_missing` | v2 | Missing keys return None |
| `test_latest_storage_legacy_does_not_read_hashed` | v1 | v1 does not read HashedStorages |
| `test_write_state_and_historical_read_hashed` | v2 | write_state + historical read roundtrip |
| `test_write_and_remove_state_roundtrip_legacy` | v1 | Full write + remove_state_above roundtrip |
| `test_unwind_storage_hashing_with_hashed_state` | v2 | unwind_storage_hashing with pre-hashed keys |
| `test_unwind_storage_hashing_legacy` | v1 | unwind_storage_hashing with plain keys |
| `from_reverts_with_hashed_state` | v2 | Trie from_reverts passthrough |
| `from_reverts_legacy_keccak_hashes_all_keys` | v1 | Trie from_reverts hashes all keys |
| `test_hashed_storage_from_reverts_legacy` | v1 | Storage root reverts |
| `test_hashed_storage_from_reverts_hashed_state` | v2 | Storage root reverts with pre-hashed keys |
| `test_maybe_hash_key_*` (3 tests) | both | Unit tests for maybe_hash_key |

**No critical coverage gaps for v2 production paths.**
