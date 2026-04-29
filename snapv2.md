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

### Sync Algorithm

1. **Download headers and identify the chain head.** The CL sends `new_payload` / `forkchoiceUpdated` — the engine knows HEAD immediately.
2. **Pick a pivot block P** sufficiently behind HEAD (e.g., HEAD−64) to reduce the likelihood of P being reorged while remaining recent enough that serving peers still hold its state.
3. **Bulk download flat state at P** via `GetAccountRange` / `GetStorageRanges` / `GetByteCodes`. Write directly to `HashedAccounts` / `HashedStorages` / `Bytecodes`.
4. **If P becomes stale during download** (serving peers no longer hold state at P — detected by empty responses mid-range), fetch BALs for blocks P+1..P' via `GetBlockAccessLists` (where P' is the new pivot), apply the diffs locally, update root_hash, and continue download at P'.
5. **While state is being downloaded, the chain advances** and the pivot may need to switch from P to P+K. BALs for the gap are either already buffered (from CL `new_payload` events received during download) or fetched from peers. Each BAL is verified against its header's `block_access_list_hash` before application.
6. **If the pivot advances further during step 5**, repeat for the newly produced blocks.
7. **Recompute and verify the state root** against the latest header.

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

## Architecture

### Two-Phase Design

Snap sync uses two complementary mechanisms:

1. **HeaderStage (historical)** — downloads headers 1..pivot via the existing `HeaderStage` from the staged sync pipeline. This is a one-shot batch operation that populates static files with all historical headers. Fast and well-tested.

2. **Engine-driven orchestrator (live)** — downloads state at the pivot, ingests blocks/headers/BALs for the range beyond the pivot as the chain advances, and verifies the final state root. This is a live, reactive process driven by FCU events from the CL.

### Why This Split

The staged sync pipeline is batch + sequential — perfect for downloading a known range of headers (1..pivot) into static files. But snap sync's state download is **live and reactive**: the chain advances during download, pivot may need to advance, and BALs must be ingested continuously. This is the engine's domain.

Key constraint: **during snap sync the engine returns `SYNCING` to the CL.** The CL may not send `new_payload` while the node is syncing — we can only rely on `forkchoiceUpdated` for chain advancement signals. The orchestrator must actively fetch headers and BALs from peers rather than waiting for the CL to provide them.

### Engine Integration

The engine tree already has block download primitives (`BasicBlockDownloader`, `FullBlockClient`, `DownloadRequest`) that fetch full blocks (header + body) from peers when encountering unknown block hashes. During snap sync, instead of dropping downloaded blocks (current behavior when backfill is active), the engine forwards them to the orchestrator.

```
              Consensus Layer
                    │
         FCU(head_hash) only
         (no new_payload during SYNCING)
                    │
                    ▼
          ┌─────────────────────┐
          │     Engine Tree     │
          │                     │
          │  FCU → download     │──── SnapSyncEvent::NewHead{hash}
          │  machinery fetches  │     (just the hash from FCU)
          │  unknown blocks     │
          │  from peers         │──── Downloaded SealedBlocks
          │                     │     (header + body, includes BAL?)
          └──────────┬──────────┘
                     │
                     ▼
          ┌──────────────────────┐
          │    Orchestrator      │
          │                      │
          │  Buffers blocks      │
          │  Orders by number    │
          │                      │
          ├──────────┬───────────┤
          │          │           │
          ▼          ▼           ▼
     Persistence   BAL Store   State Download
     (headers to   (for state  (GetAccountRange,
      static       healing)    GetStorageRanges,
      files from              GetByteCodes at
      pivot+1)                pivot root)
```

### Orchestrator Algorithm

```
Phase A — Header Pipeline (batch):
  Receive BackfillAction::StartSnapSync(target_hash)
  Resolve target_number from peers via HeadersClient
  Pick pivot P = target_number − 64
  Run HeaderStage with NoopConsensus: downloads headers 1..P
    → headers in static files, HeaderNumbers in DB, checkpoint set
  Headers 0..P now available via normal provider lookups

Phase B — State Download (live, engine-driven):
  Read pivot root from local header (static files)
  Clear HashedAccounts / HashedStorages / Bytecodes / trie tables

  Bulk download at pivot:
    GetAccountRange / GetStorageRanges / GetByteCodes at pivot root
    Write directly to HashedAccounts / HashedStorages / Bytecodes

  Concurrently, ingest blocks from engine:
    Engine receives FCU → downloads unknown blocks from peers
    Downloaded blocks forwarded to orchestrator
    Orchestrator buffers headers + BALs
    Pipes headers to persistence (extends static files from pivot+1)
    Stores BALs for state healing

  Pivot advancement (when pivot becomes stale):
    Fetch BALs for P+1..P' (from buffer or peers via GetBlockAccessLists)
    Resolve block hashes via buffered headers or HeadersClient
    Apply BAL diffs to HashedAccounts / HashedStorages / Bytecodes
    P' becomes new pivot, continue bulk download at P'.state_root

  Final BAL catch-up:
    Apply BALs from initial_pivot+1 to known_head (not current pivot)
    Initial pivot is the block at which bulk download started
    When pivot advances mid-download, MDBX state is mixed (some accounts at
    initial pivot, some at advanced pivot). BALs set absolute values, so
    applying all blocks from initial_pivot+1 in order heals everything.

  Verification:
    Compute state root from HashedAccounts / HashedStorages
    Verify against header(final_block).state_root
    Report SnapSyncOutcome to engine
```

### Engine → Orchestrator Protocol

```
SnapSyncEvent::NewHead { head_hash: B256 }
    — sent on forkchoiceUpdated; just the hash (orchestrator resolves number from peers)

SnapSyncEvent::DownloadedBlock { block: SealedBlock }
    — forwarded from engine's block download machinery (header + body)
    — during snap sync, on_downloaded_block forwards instead of dropping

SnapSyncOutcome { synced_to, block_hash, state_root }
    — reported back to engine on completion
```

### After Snap Sync Completes

1. Headers 0..final_block are in static files (historical via HeaderStage, recent headers persisted via PersistenceTask during sync)
2. Hashed state leaves (HashedAccounts, HashedStorages, Bytecodes) verified at final_block
3. Stage checkpoints set: Execution/AccountHashing/StorageHashing/Bodies/SenderRecovery/Histories → snap target. MerkleExecute + Finish left at 0.
4. Normal backfill pipeline triggered — only MerkleExecute + Finish run. MerkleExecute builds AccountsTrie/StoragesTrie from the hashed leaves. No state root computation needed during snap sync itself (BALs populate leaves, MerkleExecute builds the trie).
5. Node transitions to normal live sync mode

### Crash Resistance

A "snap sync in progress" marker is written to DB before clearing hashed state. If the node crashes mid-sync:

1. On restart, engine finds the marker
2. Wipes all hashed state (partial state is untrusted)
3. Restarts snap sync from scratch

Resume-from-midpoint is a future optimization (would require persisting download progress: last hash range, current pivot, etc.).

## Implementation Status

### What Works End-to-End

The full snap/2 sync pipeline is functional:

1. **Fresh node detection** — engine detects `best_block_number == 0` and emits `BackfillAction::StartSnapSync`
2. **Phase A (HeaderStage)** — downloads headers 1..pivot via `ReverseHeadersDownloader` + `HeaderStage`, writes to static files
3. **Phase B (SnapSyncOrchestrator)** — bulk state download at pivot root, BAL catch-up for blocks pivot+1..head
4. **Stale pivot recovery** — when the serving peer's lookback no longer covers the pivot root, the orchestrator advances the pivot and resumes download
5. **Static file segment advancement** — after snap sync, Transactions/TransactionSenders/Receipts/ChangeSets segments are advanced to the snap target block so persistence doesn't fail
6. **Stage checkpoint writing** — Bodies/SenderRecovery/Execution/Hashing/History stages set to snap target; MerkleExecute + Finish left at 0
7. **MerkleExecute verification** — pipeline runs MerkleExecute to rebuild the trie from hashed leaves, verifying state root against headers
8. **Post-snap live sync** — engine transitions to normal block processing, downloads blocks after the snap target, and persists them

### Passing E2E Tests

- **`can_snap_sync_catch_up`** — Node A builds 20 blocks, advances 15 more while Node B snap syncs. Tests full pipeline: snap download → BAL catch-up → MerkleExecute → live sync to block 35. ~50s in debug.
- **`can_snap_sync_stale_pivot`** — Node A builds 180 blocks so the initial pivot (block 4) falls outside the 128-block serving lookback. Tests stale pivot recovery: orchestrator advances pivot 4 → 14 → 24 → ... until it reaches a servable root, then completes sync. ~225s in debug.
- **`can_snap_sync_frozen_head`** — `#[ignore]`, requires serving in-memory state (frozen head = persistence never flushes last ~2 blocks)

### Completed Tasks

| Task | Description | Status |
|---|---|---|
| Initial pivot tracking | Orchestrator saves `initial_pivot` and uses it for BAL healing range | ✅ Done |
| Stage checkpoints | All non-trie stages set to snap target after Phase B | ✅ Done |
| Static file advancement | Transactions/Senders/Receipts/ChangeSets segments advanced via `ensure_at_block` | ✅ Done |
| Pipeline trigger | `on_snap_sync_finished` triggers backfill pipeline for MerkleExecute + Finish | ✅ Done |
| HeaderStage (Phase A) | `SnapSyncController` runs `HeaderStage` for headers 1..pivot | ✅ Done |
| Two-phase architecture | Phase A in `SnapSyncController`, Phase B in `SnapSyncOrchestrator` | ✅ Done |
| Engine block forwarding | `on_new_payload` / `on_downloaded_block` forward events to orchestrator | ✅ Done |
| BAL data flow fix | V4 payload construction forced when BAL present; slot_number fix | ✅ Done |
| Skip move_to_static_files | Pipeline skips static file move for storage v2 nodes | ✅ Done |
| Stale pivot E2E test | `can_snap_sync_stale_pivot` exercises full recovery path | ✅ Done |

### Current Key Files

| Component | Location |
|---|---|
| SnapClient trait | `crates/net/p2p/src/snap/client.rs` |
| BlockAccessListsClient | `crates/net/p2p/src/block_access_lists/client.rs` |
| HeadersClient | `crates/net/p2p/src/headers/client.rs` |
| BalCache / BalStore | `crates/rpc/rpc-engine-api/src/bal_cache.rs`, `crates/storage/storage-api/src/bal.rs` |
| Snap serving | `crates/node/builder/src/snap_provider.rs`, `crates/net/network/src/snap_requests.rs` |
| BAL serving | `crates/net/network/src/eth_requests.rs` (EthRequestHandler.bal_store) |
| Snap sync crate | `crates/engine/snap/src/{lib,orchestrator,bal,download,pivot}.rs` |
| Backfill / SnapSyncController | `crates/engine/tree/src/backfill.rs` |
| Engine tree | `crates/engine/tree/src/tree/mod.rs` |
| Engine launch | `crates/engine/tree/src/launch.rs` |
| E2E tests | `crates/ethereum/node/tests/e2e/p2p.rs` |

### Refactoring Plan

The implementation was iterated across many sessions, accumulating structural debt. The algorithms are correct; the issues are **module boundaries** and **generic complexity**.

**P0 — Delete dead code**
- Remove `crates/e2e-test-utils/src/snap.rs` (`DbSnapStateProvider` — unused)

**P1 — Fix `backfill.rs` shape** (biggest code quality issue)
- Split into `backfill/mod.rs`, `backfill/pipeline.rs`, `backfill/controller.rs`
- Introduce `SnapBackfill` trait to erase generics (eliminates 3x repeated 15-line where clauses)
- Move `SnapSyncController` to `crates/engine/snap/src/service.rs` (snap domain logic, not tree logic)

**P2 — Extract snap integration from `tree/mod.rs`**
- Create `crates/engine/tree/src/tree/snap.rs`
- Move `fresh_node` detection, `snap_events_tx` handling, event forwarding, `on_snap_sync_finished`
- Collapse fields into `SnapTreeState` struct

**P3 — Split `download.rs` by concern**
- `download.rs` — network download loops only
- `storage.rs` — MDBX write helpers (clear/read/write hashed state)
- `finalize.rs` — stage checkpoints + static file advancement

**P4 — Consider moving `BalCache`** (lower priority)
- `bal_cache.rs` implements `BalStore` (shared infra), lives in `rpc-engine-api` (RPC-specific)
- Move to `crates/storage/provider/` if dependency graph allows
- Defer if awkward — `BalStoreHandle` abstraction already limits damage

### Known Limitations

- **Frozen-head serving** — engine keeps ~2 blocks in memory before batch-persisting, so snap serving lags. If no new blocks arrive, the last ~2 blocks never flush. Fix: serve from BlockchainProvider (MDBX + in-memory overlay).
- **No crash resume** — if node crashes mid-snap-sync, it restarts from scratch. Future: persist download progress.
- **No reorg handling during snap sync** — if a reorg occurs past the pivot during download, behavior is undefined. Future: detect via NewHead events and re-evaluate pivot.

### Future Work

- **Proof-based trie population:** `GetAccountRange`/`GetStorageRanges` responses include Merkle boundary proofs. If verified, these proof nodes can be inserted directly into `AccountsTrie`/`StoragesTrie`, eliminating the need to recompute the entire trie from scratch.
- **Historical root serving via sorted overlay:** To serve `GetAccountRange` at historical roots (HEAD−64), reverse-apply BALs to build a `HashedPostState` overlay and merge-iterate with MDBX cursors. Infrastructure (`HashedPostState`/`HashedPostStateSorted`) already exists.
- **Resume from checkpoint:** Persist download progress (last hash range, current pivot) so crash recovery can resume mid-download instead of restarting from scratch.
- **Reorg handling during snap sync:** If a reorg occurs past the pivot during download, detect via NewHead events and re-evaluate pivot selection.

### References

- [EIP-7928: Block-Level Access Lists](https://eips.ethereum.org/EIPS/eip-7928)
- [EIP-8189: snap/2 — BAL-Based State Healing](https://eips.ethereum.org/EIPS/eip-8189)
- [Snap/1 Protocol Spec](https://github.com/ethereum/devp2p/blob/master/caps/snap.md)
