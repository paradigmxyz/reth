# Invariants: `reth-trie-common`

This file declares the invariants for `reth-trie-common`. Each invariant is a
statement that MUST hold at all times. Changes that violate an invariant MUST be
rejected or the invariant MUST be explicitly updated with justification.

Invariants are machine-readable: an LLM CI agent parses this file and checks
every PR diff against the rules below. They are also the source of truth for
property-based test generation.

---

## Format

```
### INV-<module>-<number>: <short name>
Scope: <file or module path>
Severity: critical | high | medium
Added: <date> | Reason: <link or one-liner>

<plain-english statement of the invariant>
```

---

## PrefixSet

### INV-PREFIX-001: Freeze produces sorted, deduplicated keys
Scope: `src/prefix_set.rs` — `PrefixSetMut::freeze`
Severity: critical
Added: 2025-02-07 | Reason: Trie traversal correctness depends on binary-search over keys

After calling `PrefixSetMut::freeze()`, the resulting `PrefixSet` MUST contain
keys that are sorted in ascending order and contain no duplicates. The `freeze`
method is the ONLY code path that transitions mutable to immutable; no other
method may produce a `PrefixSet`.

### INV-PREFIX-002: `all` flag supersedes key membership
Scope: `src/prefix_set.rs` — `PrefixSet::contains`
Severity: critical
Added: 2025-02-07 | Reason: `all` is a sentinel meaning "recompute everything"

When `PrefixSet.all` is `true`, `contains()` MUST return `true` for ANY input,
regardless of the contents of `keys`. The `all` flag propagates through
`extend`: if either operand has `all == true`, the result MUST also have
`all == true`.

### INV-PREFIX-003: Cursor monotonicity in `PrefixSet::contains`
Scope: `src/prefix_set.rs` — `PrefixSet::contains`
Severity: high
Added: 2025-02-07 | Reason: Perf optimization from Silkworm — PR #2417

`PrefixSet::contains` maintains an internal `index` cursor that advances
forward during sequential lookups. The cursor MAY be rewound (moved backward)
if the queried prefix is less than the key at the current cursor position, but
it MUST NEVER skip past a matching key. In other words: the cursor is an
optimization, not a filter — it must not cause false negatives.

---

## Nibbles / StoredNibbles

### INV-NIBBLES-001: Compact roundtrip identity
Scope: `src/nibbles.rs` — `StoredNibbles`, `StoredNibblesSubKey`
Severity: critical
Added: 2025-02-07 | Reason: Database serialization correctness

For any `StoredNibbles` value `n`:
`StoredNibbles::from_compact(n.to_compact(buf), len) == n`.
The same applies to `StoredNibblesSubKey`. Compact encoding MUST be
lossless and roundtrip-safe.

### INV-NIBBLES-002: SubKey fixed-width encoding
Scope: `src/nibbles.rs` — `StoredNibblesSubKey::to_compact`
Severity: high
Added: 2025-02-07 | Reason: Database key alignment

`StoredNibblesSubKey::to_compact` MUST always write exactly 65 bytes
(64 nibble bytes + 1 length byte). The nibble data is right-padded with zeros.
The length byte at position 64 records the actual nibble count and MUST
be ≤ 64.

---

## Trie Updates

### INV-UPDATES-001: Updated nodes take precedence over removed nodes
Scope: `src/updates.rs` — `TrieUpdates::extend`, `into_sorted`, `clone_into_sorted`
Severity: critical
Added: 2025-02-07 | Reason: Prevents stale deletions from overriding live data

When a path appears in both `account_nodes` (updated) and `removed_nodes`
(deleted), the updated entry MUST win. Concretely: `extend` removes from
`account_nodes` any path that appears in `other.removed_nodes`, and
`into_sorted` / `clone_into_sorted` strip from `removed_nodes` any path
present in `account_nodes`.

### INV-UPDATES-002: Empty nibble paths are excluded
Scope: `src/updates.rs` — `exclude_empty`, `exclude_empty_from_pair`
Severity: high
Added: 2025-02-07 | Reason: Empty path = root node, handled separately

Node paths with zero-length nibbles MUST NOT appear in `account_nodes` or
`removed_nodes`. The `exclude_empty` / `exclude_empty_from_pair` filters
enforce this at every insertion boundary.

### INV-UPDATES-003: Sorted representation is actually sorted
Scope: `src/updates.rs` — `TrieUpdatesSorted`, `StorageTrieUpdatesSorted`
Severity: critical
Added: 2025-02-07 | Reason: Merge algorithms assume sorted input

`TrieUpdatesSorted.account_nodes` and `StorageTrieUpdatesSorted.storage_nodes`
MUST be sorted by nibble path in ascending order. Any code that constructs a
`*Sorted` variant MUST sort before returning.

---

## Ordered Trie Root Builder

### INV-ORDERED-001: Builder equivalence with `ordered_trie_root`
Scope: `src/ordered_root.rs` — `OrderedTrieRootEncodedBuilder`
Severity: critical
Added: 2025-02-07 | Reason: Receipt / transaction root correctness

For any sequence of items, `OrderedTrieRootEncodedBuilder` MUST produce the
exact same root hash as `alloy_trie::root::ordered_trie_root_encoded` given
the same items, regardless of push order.

### INV-ORDERED-002: No duplicate or out-of-bounds indices
Scope: `src/ordered_root.rs` — `OrderedTrieRootEncodedBuilder::push`
Severity: high
Added: 2025-02-07 | Reason: Double-counting items corrupts the trie

`push(index, _)` MUST reject (return `Err`) if `index >= len` or if an item
at `index` was already pushed. `push_unchecked` asserts these in debug mode.

### INV-ORDERED-003: Empty builder yields `EMPTY_ROOT_HASH`
Scope: `src/ordered_root.rs` — `OrderedTrieRootEncodedBuilder::finalize`
Severity: medium
Added: 2025-02-07 | Reason: Consistency with Ethereum spec

An `OrderedTrieRootEncodedBuilder` with `len == 0` MUST return
`EMPTY_ROOT_HASH` on `finalize()`, matching the Ethereum empty trie root.

---

## MultiProof

### INV-PROOF-001: Extend merges, never drops
Scope: `src/proofs.rs` — `MultiProof::extend`
Severity: critical
Added: 2025-02-07 | Reason: Incomplete proofs cause verification failures

`MultiProof::extend(other)` MUST retain all proof nodes from both `self` and
`other`. After extend, every key that was present in either proof MUST be
present in the result. For storage proofs at the same hashed address, subtrees
are merged.

### INV-PROOF-002: `retain_difference` is idempotent on disjoint sets
Scope: `src/proofs.rs` — `MultiProofTargets::retain_difference`
Severity: high
Added: 2025-02-07 | Reason: Prefetch optimization correctness

`targets.retain_difference(other)` where `other` has no overlap with `targets`
MUST leave `targets` unchanged. Removing the same `other` twice MUST be
equivalent to removing it once.

### INV-PROOF-003: Chunking preserves all targets
Scope: `src/proofs.rs` — `ChunkedMultiProofTargets`
Severity: high
Added: 2025-02-07 | Reason: Parallel proof fetching must not lose targets

The union of all chunks produced by `MultiProofTargets::chunks(size)` MUST
equal the original `MultiProofTargets`. No account or slot may be lost or
duplicated across chunks.

---

## TrieAccount

### INV-ACCOUNT-001: Default GenesisAccount → empty trie account
Scope: `src/account.rs`
Severity: medium
Added: 2025-02-07 | Reason: Genesis block correctness

Converting a default `GenesisAccount` to `TrieAccount` MUST yield
`nonce == 0`, `balance == 0`, `storage_root == EMPTY_ROOT_HASH`, and
`code_hash == KECCAK_EMPTY`.

### INV-ACCOUNT-002: Zero storage values produce EMPTY_ROOT_HASH
Scope: `src/account.rs`
Severity: medium
Added: 2025-02-07 | Reason: EIP-161 compatibility

If all storage values in a `GenesisAccount` are zero, the resulting
`TrieAccount.storage_root` MUST equal `EMPTY_ROOT_HASH`.

---

## Serialization (all types)

### INV-SERDE-001: Roundtrip identity for all Compact types
Scope: crate-wide
Severity: critical
Added: 2025-02-07 | Reason: Database integrity

For every type implementing `reth_codecs::Compact`:
`T::from_compact(t.to_compact(buf), len) == t`. This applies to
`StoredNibbles`, `StoredNibblesSubKey`, `StoredSubNode`, `BranchNodeCompact`,
and all trie update types.

### INV-SERDE-002: Roundtrip identity for serde/bincode types
Scope: crate-wide
Severity: critical
Added: 2025-02-07 | Reason: Network and snapshot integrity

For every type deriving `serde::Serialize + serde::Deserialize`:
`deserialize(serialize(t)) == t`. This covers both JSON (serde) and bincode
(serde_bincode_compat) paths.
