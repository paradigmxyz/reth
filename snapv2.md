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
3. **Snap leaf payload types** — `AccountData` carries `TrieAccount` internally and owns the snap slim account body codec at the wire boundary; storage value helpers live on `StorageData`; engine snap no longer carries standalone account/storage RLP encode/decode helpers.
4. **Snap/2 BAL exchange** — `SnapClient` exposes `get_snap_block_access_lists`, `PeerRequest` has a separate `GetSnapBlockAccessLists` variant, and `SnapRequestHandler` serves BALs through the snap request path.
5. **Snap-owned serving logic** — provider-backed snap serving lives in `reth-engine-snap::serve::ProviderSnapState`; the network crate owns only the generic `SnapStateProvider` trait and request handler wiring.
6. **Snap-owned lifecycle logic** — Phase A / Phase B orchestration lives in `reth-engine-snap::controller::SnapSyncController`; `engine-tree` only starts/polls the controller and forwards events.
7. **Tree snap state isolation** — `EngineApiTreeHandler` stores snap-specific state in `SnapTreeState` under `tree/snap.rs` rather than carrying loose `fresh_node` / `snap_events_tx` fields.
8. **BAL verification** — BAL catch-up fetches missing BALs through snap/2, decodes them with alloy RLP, computes the EIP-7928 BAL hash, and compares it with the header's `block_access_list_hash` before applying state changes.
9. **Hashed state hardening** — snap clears `HashedAccounts`, `HashedStorages`, `AccountsTrie`, `StoragesTrie`, and `Bytecodes`; hashed account/storage writes are adapted into `HashedPostStateSorted` and applied through the provider `StateWriter`, and bytecodes are written through `StateWriter::write_bytecodes`, so snap no longer writes those storage tables directly.

## Snap/1 Shared-Message Compliance Plan

EIP-8189 keeps snap/1 messages `0x00..0x05` unchanged. This branch now implements the shared account, storage, and bytecode range rules needed by snap/2, including snap slim account bodies and range proof verification.

### 1. Snap Account Body Encoding — implemented

- Replace full `TrieAccount` account bodies on the snap wire with the snap slim account body codec.
- Encode `storage_root == EMPTY_ROOT_HASH` as RLP empty list and `code_hash == KECCAK_EMPTY` as RLP empty list, matching snap/1's account body format.
- Keep typed conversions to/from `TrieAccount` so trie proof verification and hashed DB writes still use the canonical account representation internally.
- Add roundtrip tests that assert empty storage/code fields use the slim encoding and decode back to canonical `TrieAccount` values.

### 2. Account Range Serving Rules — implemented

- If the serving node has the requested `rootHash`, always return a proof for `startingHash` unless the response is the entire account trie.
- If no accounts exist in `[startingHash, limitHash)`, return the first account after `limitHash` when one exists, as required by snap/1.
- Keep `responseBytes` as a soft cap, but ensure a non-empty servable range returns at least one account.
- Preserve hash monotonicity in responses and include proof nodes for the requested origin plus the last returned account.

### 3. Account Range Verification — implemented

- Verify that returned account hashes are strictly increasing and start at or after the requested origin.
- Verify proof-bearing responses with the internal snap range verifier, so the proof must establish that returned leaves are complete between the requested origin and right boundary, not just individually valid.
- Treat empty account responses as stale only when the serving peer lacks the requested root, not as successful end-of-state unless the proof establishes the end.
- Feed account ranges into the verifier with canonical trie account RLP values, while keeping the snap slim account body codec only at the wire boundary.
- Reject skipped-middle account ranges with adversarial proof tests.

### 4. Storage Range Serving Rules — implemented

- If the serving node lacks the requested state root or any requested account hash, return an empty reply.
- For multi-account storage requests, return full storage ranges for each account until the response limit is hit; only the last included account may be partial, and only then attach a proof.
- For single-account continuation requests, honor `startingHash` / `limitHash`, attach a proof whenever the returned range is partial, and allow proof-free responses only when the entire requested storage range is returned.
- Preserve storage slot hash ordering and skip zero-valued slots.

### 5. Storage Range Verification — implemented

- Verify every storage response is ordered and maps one-to-one to the requested account hashes.
- When a proof is present, verify the full partial range with the internal snap range verifier and RLP-encoded storage slot values.
- When no proof is present, recompute the full storage root for that account before trusting a complete storage range, or otherwise require a proof.
- Reject partial-looking storage responses without proofs instead of silently writing them.
- Continue recomputing full storage roots for proof-free complete storage ranges.
- Ensure continuation chunks are accumulated and verified before any truncated account storage is written.

### 6. Bytecode Response Matching — implemented

- Snap/1 bytecode responses skip unavailable hashes, so the downloader must not zip response index to request index.
- Validate each returned bytecode by `keccak256(code)` and match it to one of the requested hashes.
- Require returned bytecodes to be a request-order subsequence while still allowing unavailable hashes to be skipped.
- Empty bytecode responses should mean "none of these codes are available" for that request, not automatically "stale root".

### 7. Tests And Validation — passing locally

- Add unit tests for slim account encoding, account range edge cases, storage partial/full proof behavior, and bytecode hash matching.
- Keep `run_snap_test.sh` running both active snap e2e tests: `can_snap_sync_catch_up` and `can_snap_sync_stale_pivot`.
- After each protocol-significant change, run the focused crate tests plus `./run_snap_test.sh`.

Latest validation:

- `cargo test -p reth-eth-wire-types snap_account`
- `cargo test -p reth-engine-snap proof`
- `cargo test -p reth-engine-snap download::tests`
- `cargo check -p reth-engine-snap`
- `git diff --check`
- `zepter`
- `make lint-toml`
- `./run_snap_test.sh`

### 8. Range Proof Hardening — implemented

The old endpoint checks proved that the requested origin and returned right boundary were individually valid, but that was not enough to prove there were no omitted leaves between them. The downloader now uses the private `crates/engine/snap/src/proof.rs` module:

- `verify_range_proof(root, origin, leaves, proof)` is generic over `(hashed_key, encoded_value)`, so account and storage ranges share the same verifier.
- The verifier reconstructs the trie frontier from returned leaves plus proof subtrees outside the proven range, then compares the reconstructed root with the expected account/storage root.
- Account ranges feed canonical `TrieAccount` RLP values into the verifier.
- Storage ranges feed canonical RLP-encoded `U256` slot values into the verifier.
- Tests cover complete boundary multiproofs, proof-free full trie responses, omitted interior leaves, empty tail proofs, and empty ranges that hide a right-side leaf.

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
| Snap range proof verification | `crates/engine/snap/src/proof.rs` |
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
| Snap account/storage proofs | Provider-backed serving now emits boundary proof nodes for account ranges and partial storage ranges; the downloader reconstructs the proven range frontier and rejects omitted interior leaves before writing data |
| Served account storage roots | `AccountRange` bodies now carry storage roots computed from the same historical hashed-state overlay used to serve the account and storage leaves |

### Remaining Debt

- **No crash resume.** If the node crashes mid-snap-sync, it should wipe partial hashed state and restart. Resume-from-midpoint would require persisting range cursor, pivot, and downloaded bytecode/account progress.
- **No reorg handling during snap sync.** If a reorg crosses the pivot, the current implementation does not collect old-fork BALs and re-fetch affected ranges as EIP-8189 describes.
- **Frozen-head serving is still limited.** Provider-backed serving reconstructs recently persisted state with changesets; it does not yet serve the final unflushed in-memory blocks.
- **BAL cache placement remains debatable.** `BalStoreHandle` abstracts the store, but some implementation still lives near Engine API/RPC concerns. Moving shared BAL cache implementation into storage/provider may be cleaner later.

### Future Work

- **Trie population from verified proofs:** optionally insert verified proof nodes into `AccountsTrie`/`StoragesTrie` to reduce MerkleExecute work.
- **Historical root serving cache:** Cache `HashedPostStateSorted` overlays per served pivot so repeated account/storage range requests do not rebuild reverse diffs.
- **Peer reliability tracking for BALs:** Peers returning `0x80` for available BALs should be deprioritized separately from ordinary empty snap state responses.
- **Reorg handling during snap sync:** Implement the EIP-8189 old-fork/new-fork BAL procedure, and restart if required orphaned BALs are unavailable.

### References

- [EIP-7928: Block-Level Access Lists](https://eips.ethereum.org/EIPS/eip-7928)
- [EIP-8189: snap/2 — BAL-Based State Healing](https://eips.ethereum.org/EIPS/eip-8189)
- [Snap/1 Protocol Spec](https://github.com/ethereum/devp2p/blob/master/caps/snap.md)
