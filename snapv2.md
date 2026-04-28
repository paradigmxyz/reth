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

### Completed

**Network plumbing:**
- `FetchClient` implements `SnapClient` + `BlockAccessListsClient` + `HeadersClient`
- Snap capability negotiation in RLPx handshake
- `GetBlockAccessLists`/`BlockAccessLists` wire messages (eth/71)
- `SnapRequestHandler` serving `GetAccountRange`/`GetStorageRanges`/`GetByteCodes` via MDBX cursors
- `EthRequestHandler` has `bal_store` field for BAL serving (via `BalStoreHandle`)

**BAL infrastructure:**
- `BalCache` buffers BALs from `new_payload` in memory (`crates/rpc/rpc-engine-api/src/bal_cache.rs`)
- `BalStore` trait for retrieval by hash or range (`crates/storage/storage-api/src/bal.rs`)

**Snap serving:**
- `ProviderSnapState` wraps `BlockchainProvider` to serve snap requests
- `current_state_root()` reads `StageId::Execution` checkpoint from MDBX — this is written atomically with hashed state by the persistence task (`save_blocks` → `update_pipeline_stages` in the same MDBX commit), so it exactly tracks which block's hashed state is in `HashedAccounts`
- Note: `best_block_number()` races ahead (in-memory canonical tip), `last_block_number()` tracks static files — neither matches MDBX hashed state
- MDBX cursor iteration for account/storage/bytecode data
- Limitation: the engine keeps ~2 blocks in memory before batch-persisting, so the serving node's stage checkpoint lags ~2 blocks behind the canonical tip. A frozen-head scenario (no new blocks arriving) means those last ~2 blocks never get flushed. Future fix: serve from BlockchainProvider (MDBX + in-memory overlay) instead of raw MDBX cursors

**Engine-driven snap sync crate (`crates/engine/snap/`):**
- `lib.rs` — `SnapSyncEvent`, `SnapSyncOutcome`, `SnapSyncError` types
- `bal.rs` — BAL diff logic: `bal_to_state_diff`, `merge_account_diff`
- `download.rs` — Account/storage/bytecode download loops with MDBX write helpers
- `pivot.rs` — Pivot tracking, BAL buffering, `advance_pivot`, `get_bal_bytes`
- `orchestrator.rs` — `SnapSyncOrchestrator` main async loop (bootstrap → download → catch-up → verify)
- 14 unit tests passing

**Engine tree integration (partial, being reworked):**
- `BackfillAction::StartSnapSync(B256)` carries target hash from FCU
- `CombinedBackfillSync` wraps `PipelineSync` + `SnapSyncController`
- `SnapSyncController` spawns orchestrator (needs rework for two-phase design)
- Engine tree forwards `NewHead`/`NewBlock` events to orchestrator via mpsc channel
- `on_snap_sync_finished` handles completion and canonical head update

**E2E tests (`crates/ethereum/node/tests/e2e/p2p.rs`):**
- `can_snap_sync_frozen_head` — `#[ignore]`, requires serving in-memory state (frozen head = persistence never flushes last ~2 blocks)
- `can_snap_sync_catch_up` — live catch-up test: Node A keeps producing blocks while Node B snap syncs. Snap download + BAL healing + state root verification passes. Post-snap transition to normal pipeline not yet wired.

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
| Backfill trait | `crates/engine/tree/src/backfill.rs` |
| Engine tree | `crates/engine/tree/src/tree/mod.rs` |
| Engine launch | `crates/engine/tree/src/launch.rs` |
| Block downloader | `crates/engine/tree/src/download.rs` |
| E2E tests | `crates/ethereum/node/tests/e2e/p2p.rs` |

### Implementation Plan

**Task 1: Track initial pivot in orchestrator** *(small, ready)*

`orchestrator.rs` — save `initial_pivot_block` before the download loop. BAL healing range must be `initial_pivot + 1 ..= final_block` (not `tracker.pivot_block() + 1`). The bulk download starts at initial pivot's root; when pivot advances mid-download, the state in MDBX is mixed (early accounts at initial pivot, later accounts at advanced pivot). BALs set absolute values, so applying all blocks from initial_pivot+1 in order heals everything correctly.

**Task 2: Persist downloaded headers via PersistenceTask** *(medium)*

Block downloader provides headers for blocks ≥ pivot. These must be persisted to static files / MDBX. Send them to the PersistenceTask (a headers-only save variant, distinct from full `save_blocks`). Phase A (HeaderStage) handles 1..pivot; this handles pivot..head.

**Task 3: Set stage checkpoints after snap sync** *(medium)*

After BAL healing completes (in orchestrator or `on_snap_sync_finished`), write stage checkpoints:
- **Execution, AccountHashing, StorageHashing** → snap target (hashed leaves fully populated)
- **Bodies, SenderRecovery, AccountHistory, StorageHistory** → snap target (skip, don't care)
- **Headers** → already done by Phase A + Task 2
- **MerkleExecute** → leave at 0 (must run to build trie from leaves)
- **Finish** → leave at 0

**Task 4: Trigger backfill pipeline after snap sync** *(medium)*

`on_snap_sync_finished` kicks off a normal pipeline run. Pipeline sees all stages done except MerkleExecute + Finish. MerkleExecute builds `AccountsTrie`/`StoragesTrie` from the hashed leaves that BAL healing populated. Finish completes. Node transitions to live block processing.

**Task 5: E2E test with pivot advancement** *(small, ready)*

Update `can_snap_sync_catch_up` test: add delays so the chain keeps advancing during download, forcing the initial pivot to differ from the final block. Verify BAL healing bridges the gap correctly.

**Task 6: HeaderStage integration (Phase A)** *(medium)*

Rework `SnapSyncController` in `backfill.rs` to run a two-phase process:
1. Resolve target block from peers via `HeadersClient::get_header(target_hash)`
2. Pick pivot = target_number − PIVOT_OFFSET
3. Build `ReverseHeadersDownloader` from existing `HeadersClient` + `NoopConsensus`
4. Run `HeaderStage` (download headers 1..pivot → static files + HeaderNumbers + checkpoint)
5. Then spawn `SnapSyncOrchestrator` for state download

**Task 7: Engine block forwarding during snap sync** *(small)*

During snap sync (backfill active), `on_downloaded_block` forwards blocks to the orchestrator instead of dropping them. Engine's existing download chain continues to work — blocks arrive for unknown hashes from FCU.

**Task 8: Clean up legacy pipeline snap sync** *(small)*

- Delete `crates/stages/stages/src/stages/snap_sync/` (mod.rs + bal.rs)
- Delete `SnapSyncStages` from `crates/stages/stages/src/sets.rs`
- Delete `build_snap_pipeline()` from `crates/node/builder/src/setup.rs`

**Task 9: Wire BalCache to EthRequestHandler** *(small)*

The `BalCache` (populated by EngineApi on `new_payload`) needs to be shared with `EthRequestHandler` so BALs can be served to peers. Currently defaults to `NoopBalStore`.

### Future Work

- **Proof-based trie population:** `GetAccountRange`/`GetStorageRanges` responses include Merkle boundary proofs. If verified, these proof nodes can be inserted directly into `AccountsTrie`/`StoragesTrie`, eliminating the need to recompute the entire trie from scratch.
- **Historical root serving via sorted overlay:** To serve `GetAccountRange` at historical roots (HEAD−64), reverse-apply BALs to build a `HashedPostState` overlay and merge-iterate with MDBX cursors. Infrastructure (`HashedPostState`/`HashedPostStateSorted`) already exists.
- **Resume from checkpoint:** Persist download progress (last hash range, current pivot) so crash recovery can resume mid-download instead of restarting from scratch.
- **Reorg handling during snap sync:** If a reorg occurs past the pivot during download, detect via NewHead events and re-evaluate pivot selection.

### References

- [EIP-7928: Block-Level Access Lists](https://eips.ethereum.org/EIPS/eip-7928)
- [EIP-8189: snap/2 — BAL-Based State Healing](https://eips.ethereum.org/EIPS/eip-8189)
- [Snap/1 Protocol Spec](https://github.com/ethereum/devp2p/blob/master/caps/snap.md)
