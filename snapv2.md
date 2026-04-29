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
  Pick pivot P = target_number − PIVOT_OFFSET (currently 16)
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

SnapSyncEvent::DownloadedBlock { number, hash, state_root, parent_hash }
    — forwarded from engine's block download machinery
    — during snap sync, on_downloaded_block forwards instead of inserting

SnapSyncOutcome { synced_to, block_hash }
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

### Current State

The branch now has a true `snap/2` shape instead of only preparatory BAL types:

1. **Capability negotiation** — default hello protocols advertise `snap/2`. Snap request scheduling requires peers to advertise the matching snap version, and incoming snap payloads are decoded with the negotiated version.
2. **Snap/2 wire decoding** — `snap/2` accepts messages 0x00-0x05 plus `GetBlockAccessLists` / `BlockAccessLists` at 0x08 / 0x09. The legacy snap/1 trie-node request/response types are not part of the branch API.
3. **Snap leaf payload types** — the snap wire crate owns `SlimAccount` for account bodies and storage value helpers on `StorageData`; engine snap no longer carries standalone account/storage RLP encode/decode helpers.
4. **Snap/2 BAL exchange** — `SnapClient` exposes `get_snap_block_access_lists`, `PeerRequest` has a separate `GetSnapBlockAccessLists` variant, and `SnapRequestHandler` serves BALs through the snap request path.
5. **Snap-owned serving logic** — provider-backed snap serving lives in `reth-engine-snap::serve::ProviderSnapState`; the network crate owns only the generic `SnapStateProvider` trait and request handler wiring.
6. **Snap-owned lifecycle logic** — Phase A / Phase B orchestration lives in `reth-engine-snap::controller::SnapSyncController`; `engine-tree` only starts/polls the controller and forwards events.
7. **Tree snap state isolation** — `EngineApiTreeHandler` stores snap-specific state in `SnapTreeState` under `tree/snap.rs` rather than carrying loose `fresh_node` / `snap_events_tx` fields.
8. **BAL verification** — BAL catch-up fetches missing BALs through snap/2, decodes them with alloy RLP, computes the EIP-7928 BAL hash, and compares it with the header's `block_access_list_hash` before applying state changes.
9. **Hashed state hardening** — snap clears `HashedAccounts`, `HashedStorages`, `AccountsTrie`, `StoragesTrie`, and `Bytecodes`; hashed account/storage writes are adapted into `HashedPostStateSorted` and applied through the provider `StateWriter`, and bytecodes are written through `StateWriter::write_bytecodes`, so snap no longer writes those storage tables directly.

### Passing E2E Tests

The intended snap test entrypoint is:

```bash
./run_snap_test.sh
```

The script runs both active snap sync E2E tests:

- **`can_snap_sync_catch_up`** — Node A builds 20 blocks, advances 15 more while Node B snap syncs. Tests snap download, BAL catch-up, MerkleExecute, and live sync to block 35.
- **`can_snap_sync_stale_pivot`** — Lowers the serving lookback for the test, then builds 30 blocks so the initial pivot falls outside that window. Tests stale pivot recovery by advancing the pivot until a servable root is found.

`can_snap_sync_frozen_head` remains `#[ignore]`; it needs serving from in-memory state for the last unflushed blocks.

### Current Key Files

| Component | Location |
|---|---|
| Snap wire types + version gating | `crates/net/eth-wire-types/src/snap.rs` |
| BAL response raw RLP container | `crates/net/eth-wire-types/src/block_access_lists.rs` |
| Snap client trait | `crates/net/p2p/src/snap/client.rs` |
| Snap serving trait | `crates/net/p2p/src/snap/server.rs` |
| Snap request handler | `crates/net/network/src/snap_requests.rs` |
| Snap request routing | `crates/net/network-api/src/events.rs`, `crates/net/network/src/{fetch,state,manager}.rs` |
| Session-level snap/2 encode/decode | `crates/net/network/src/session/active.rs` |
| Provider-backed snap serving | `crates/engine/snap/src/serve.rs` |
| Snap sync controller | `crates/engine/snap/src/controller.rs` |
| Snap orchestrator | `crates/engine/snap/src/orchestrator.rs` |
| BAL pivot/catch-up logic | `crates/engine/snap/src/pivot.rs`, `crates/engine/snap/src/bal.rs` |
| Hashed state writes | `crates/engine/snap/src/storage.rs` |
| Engine tree snap integration | `crates/engine/tree/src/tree/snap.rs` |
| Backfill integration | `crates/engine/tree/src/backfill.rs` |
| Node network wiring | `crates/node/builder/src/builder/mod.rs` |
| E2E test script | `run_snap_test.sh` |

### Completed Refactor

| Area | Current status |
|---|---|
| Snap provider location | Moved from `crates/node/builder/src/snap_provider.rs` to `crates/engine/snap/src/serve.rs` |
| Snap server trait | Moved to `reth-network-p2p` as `snap::server::SnapStateProvider` |
| Snap controller location | Moved out of `engine-tree/backfill.rs` into `reth-engine-snap::controller` |
| Engine tree fields | Collapsed snap-specific fields into `SnapTreeState` |
| True snap/2 client path | Added `get_snap_block_access_lists` and network routing for snap/2 BAL requests |
| Trie-node path | Removed legacy `GetTrieNodes` / `TrieNodes` wire types, client methods, request scheduling/serving, and the successful `SnapResponse::TrieNodes` path; this branch is snap/2-only |
| BAL missing-entry handling | snap/2 serving returns RLP empty string (`0x80`) for missing BALs; eth/71 keeps empty list (`0xc0`) |
| Hashed state write path | Snap batch shapes are converted into `HashedPostStateSorted` and written through `StateWriter::write_hashed_state`; bytecode batches use `StateWriter::write_bytecodes` |
| Combined backfill bounds | `CombinedBackfillSync` now depends on the snap crate's `SnapSyncControl` adapter, so client/provider bounds stay out of the combined sync impls |
| Test entrypoint | `run_snap_test.sh` now runs both active snap sync E2E tests |

### Remaining Debt

- **No crash resume.** If the node crashes mid-snap-sync, it should wipe partial hashed state and restart. Resume-from-midpoint would require persisting range cursor, pivot, and downloaded bytecode/account progress.
- **No reorg handling during snap sync.** If a reorg crosses the pivot, the current implementation does not collect old-fork BALs and re-fetch affected ranges as EIP-8189 describes.
- **Frozen-head serving is still limited.** Provider-backed serving reconstructs recently persisted state with changesets; it does not yet serve the final unflushed in-memory blocks.
- **Snap proofs are not validated yet.** Bulk account/storage download currently trusts peer responses, and provider-backed serving returns empty proof vectors. `GetAccountRange` / `GetStorageRanges` boundary proof verification needs to be implemented before this should be considered production-safe against arbitrary peers.
- **Served account storage roots are placeholders.** Provider-backed account range serving currently fills `SlimAccount.storage_root` with `EMPTY_ROOT_HASH`; production snap serving needs to derive per-account storage roots from the historical storage view and include matching boundary proofs.
- **BAL cache placement remains debatable.** `BalStoreHandle` abstracts the store, but some implementation still lives near Engine API/RPC concerns. Moving shared BAL cache implementation into storage/provider may be cleaner later.

### Future Work

- **Proof validation and trie population:** `GetAccountRange`/`GetStorageRanges` responses include Merkle boundary proofs. Verify them during download, reject invalid peers, and optionally insert verified proof nodes into `AccountsTrie`/`StoragesTrie` to reduce MerkleExecute work.
- **Historical root serving cache:** Cache `HashedPostStateSorted` overlays per served pivot so repeated account/storage range requests do not rebuild reverse diffs.
- **Peer reliability tracking for BALs:** Peers returning `0x80` for available BALs should be deprioritized separately from ordinary empty snap state responses.
- **Reorg handling during snap sync:** Implement the EIP-8189 old-fork/new-fork BAL procedure, and restart if required orphaned BALs are unavailable.

### References

- [EIP-7928: Block-Level Access Lists](https://eips.ethereum.org/EIPS/eip-7928)
- [EIP-8189: snap/2 — BAL-Based State Healing](https://eips.ethereum.org/EIPS/eip-8189)
- [Snap/1 Protocol Spec](https://github.com/ethereum/devp2p/blob/master/caps/snap.md)
