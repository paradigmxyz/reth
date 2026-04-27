# Snap/2: BAL-Based State Healing & Reth

## Background

### What is a BAL? (EIP-7928)

A **Block Access List** is a per-block structure recording every account and storage slot accessed during execution, along with their post-execution values (balance, nonce, storage writes, code changes). It's RLP-encoded and its `keccak256` hash is committed to the block header as `block_access_list_hash`. Essentially a complete, cryptographically-anchored state diff per block.

BALs are transmitted via the Engine API as a field in `ExecutionPayloadV4` and stored separately from block bodies. Nodes MUST retain BALs for at least the weak subjectivity period (~3533 epochs).

### Snap/1 Today — The Problem

A node syncing from scratch with snap/1:

1. **Download headers** via the `eth` protocol.
2. **Pick a pivot block P** (HEAD−64).
3. **Bulk download flat state at P** — `GetAccountRange` / `GetStorageRanges` / `GetByteCodes` retrieve leaf data (accounts, storage slots, bytecode) in hash-sorted ranges with Merkle boundary proofs. No internal trie nodes are downloaded.
4. **Chain advances** — while downloading (hours), blocks P+1…P+K are produced. The downloaded state becomes inconsistent (some data at P, some at P+3, etc.).
5. **Healing phase** (the bottleneck) — iteratively discover and fetch individual Merkle trie nodes via `GetTrieNodes` to fix inconsistencies. Each response reveals more missing nodes deeper in the trie. This is **unbounded and iterative** — many round trips, no upfront knowledge of how much work remains.
6. **Verify state root** against header.

### Why Reth Can't Do Snap/1

Snap/1's `GetAccountRange` requires serving nodes to **iterate accounts in hash order at any of the last ~128 blocks**. Geth solves this with a dedicated "snapshot" layer — a flat key-value store (`keccak(address) → account`) with a diff journal to reconstruct state at recent blocks.

Reth has:
- **`HashedAccounts`** (MDBX) — keyed by `keccak256(address)`, correct sort order, but only reflects **current HEAD state**. No diff journal to roll back N blocks.
- **~2 blocks of in-memory state** via `MemoryOverlayStateProvider` — far short of the ~128 block window needed.

Without a Geth-style snapshot layer or equivalent, reth cannot serve snap requests.

## Snap/2 with BALs (EIP-8189)

### Protocol Changes

- **Removed**: `GetTrieNodes` (0x06) / `TrieNodes` (0x07)
- **Added**: `GetBlockAccessLists` (0x08) / `BlockAccessLists` (0x09)
- **Unchanged**: `GetAccountRange`, `AccountRange`, `GetStorageRanges`, `StorageRanges`, `GetByteCodes`, `ByteCodes` (0x00–0x05)

### Sync Algorithm (Reth as Syncing Node)

Steps 1–3 are identical to snap/1. Reth sends `GetAccountRange` etc. **to Geth peers** (who have the snapshot layer). Reth is only a client — no snapshot layer needed on our side.

Received data maps directly to reth's existing storage:
- `[accHash, accBody]` → write to **`HashedAccounts`** (already hash-keyed ✓)
- `[slotHash, slotData]` → write to **`HashedStorages`**
- bytecodes → write to bytecode storage

**Step 5 — BAL catch-up (replaces healing):**
1. The set of blocks that advanced is known upfront: P+1 through P+K.
2. Send `GetBlockAccessLists` for those block hashes.
3. Verify each BAL: `keccak256(rlp(BAL)) == header.block_access_list_hash`.
4. Apply state diffs sequentially — direct writes to `HashedAccounts` / `HashedStorages`.
5. If the pivot advances further during catch-up, fetch more BALs and keep applying.
6. Compute state root and verify against header.

**Reorg handling:** If a reorg occurs past pivot P, collect BALs for the orphaned fork, identify entries mutated only on the old fork, delete them, re-fetch via `GetAccountRange`/`GetStorageRanges`, then apply BALs on the new canonical chain.

No design changes needed for the syncing side. The existing `HashedAccounts`/`HashedStorages` layout is the correct sort order, BAL diffs are trivial key-value updates, and trie computation already works over these tables.

### Serving Snap Requests (Reth as Serving Node)

To serve `GetAccountRange` at a historical `rootHash` (e.g., HEAD−64), reth needs to reconstruct what `HashedAccounts` looked like at that block. With BALs this becomes feasible:

#### Sorted Overlay Approach

1. **Reverse-apply BALs** from HEAD back to the pivot block, building a `HashedPostState` overlay — a `B256Map<Option<Account>>` where `None` means "account didn't exist at pivot."

2. **Sort the overlay** via `.into_sorted()` → `HashedPostStateSorted`, producing a `Vec<(B256, Option<Account>)>` sorted by hash. This pattern already exists in reth for trie computation.

3. **Merge-iterate** the sorted overlay with the MDBX `HashedAccounts` cursor (two-pointer merge):

```
MDBX cursor (HEAD state):        Sorted overlay (reverse diffs):
  0x0a1b → Alice(10 ETH)           0x0a1b → Some(Alice, 12 ETH)
  0x2f3c → Bob(3 ETH)              0x2f3c → Some(Bob, 2 ETH)
  0x7d8e → Charlie(50 ETH)         0x7d8e → Some(Charlie, 45 ETH)
  0xa1b2 → Dave(7 ETH)             0xa1b2 → None  (didn't exist)
  0xf4e5 → Eve(0.5 ETH)            0xf4e5 → Some(Eve, nonce 99)

Merge result:
  0x0a1b → Alice(12 ETH)     ← overlay wins
  0x2f3c → Bob(2 ETH)        ← overlay wins
  0x7d8e → Charlie(45 ETH)   ← overlay wins
  0xa1b2 → SKIP              ← overlay = None
  0xf4e5 → Eve(nonce 99)     ← overlay wins
  (accounts not in overlay)   ← read directly from MDBX
```

4. **Cache the overlay** for the current pivot. Rebuild when pivot advances (~every 64 blocks / ~13 minutes).

#### Why This Works

- `HashedPostState` / `HashedPostStateSorted` already exist and are used for trie computation.
- Merge iteration over sorted overlay + MDBX cursor is the same pattern reth uses in trie walks.
- BALs are already in the right format — keyed by address hash with exact post-values, so reverse application is straightforward.
- The overlay size for ~64 blocks of mainnet state changes is ~5–15 MB — easily fits in memory.

#### What's Missing

- **Reverse-apply BAL logic**: converting BAL diffs into a `HashedPostState` representing the undo.
- **Snap serving network handlers**: wiring the overlay + cursor merge to respond to `GetAccountRange` / `GetStorageRanges` / `GetByteCodes`.
- **Overlay lifecycle management**: building on pivot selection, caching, rebuilding on pivot advance.

### Existing Infrastructure in Reth

| Component | Location | Relevance |
|---|---|---|
| `HashedPostState` | `crates/trie/common/src/hashed_state.rs` | In-memory hashed state diff (`B256Map<Option<Account>>`) |
| `HashedPostStateSorted` | same file | Sorted variant (`Vec<(B256, Option<Account>)>`) for merge iteration |
| `HashedStorage` / `HashedStorageSorted` | same file | Same pattern for storage slots |
| `MemoryOverlayStateProvider` | `crates/chain-state/src/memory_overlay.rs` | Existing overlay pattern (point-lookups only, not sorted iteration) |
| Snap wire types | `crates/net/eth-wire-types/src/snap.rs` | `GetAccountRangeMessage`, `SnapProtocolMessage`, etc. already defined |
| `SnapClient` trait | `crates/net/p2p/src/snap/client.rs` | Client-side interface already defined |
| `HashedAccounts` table | `crates/storage/db-api/src/tables/mod.rs` | MDBX table, hash-sorted, supports cursor iteration |

### References

- [EIP-7928: Block-Level Access Lists](https://eips.ethereum.org/EIPS/eip-7928)
- [EIP-8189: snap/2 — BAL-Based State Healing](https://eips.ethereum.org/EIPS/eip-8189)
- [Snap/1 Protocol Spec](https://github.com/ethereum/devp2p/blob/master/caps/snap.md)
