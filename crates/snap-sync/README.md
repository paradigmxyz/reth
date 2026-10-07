# reth-snap-sync

snap/2 state synchronization, as specified by [EIP-8189](https://eips.ethereum.org/EIPS/eip-8189).

Instead of executing every block from genesis, a node downloads the state of a recent block from
peers and verifies it against that block's state root. snap/2 keeps that state current with
[EIP-7928 block access lists](https://eips.ethereum.org/EIPS/eip-7928) (BALs), which record every
field each block changes, replacing snap/1's trie-node healing.

This crate owns the synchronization logic and its progress. Requests and proof checks come from
`reth-downloaders`, and running the sync inside a node is left to node integration.

## How a sync runs

`SnapBootstrap` drives one sync from start to hand-off:

1. **Pick a pivot.** A recent finalized block is preferred, since it cannot be reorged; otherwise
   the pivot sits 64 blocks behind the head by default, the EIP's example distance. With no eligible
   block the sync waits.
2. **Download the state at the pivot.** Accounts are fetched in key-order ranges, proved against the
   pivot's state root. A range commits only once its contracts' storage and code are persisted,
   whether supplied with it or stored beforehand. Storage batches are prefetched with at most four
   requests or verified responses retained, and commit in key order. Partial contracts finish before
   later batches commit. Storage and bytecode download concurrently; neither advances account
   coverage until both finish. Their commits share an asynchronous writer gate to avoid MDBX's
   busy retry sleeps; a running commit retains the gate even if its waiting future is dropped.
   The network permits up to four Snap requests per provider, while
   other downloads retain their single-request scheduling. Bytecode presence checks avoid copying
   blobs during download, account commits, and the completeness scan.
3. **Advance the pivot.** Peers only keep recent state, so once the pivot lags more than 96 blocks
   by default the sync re-anchors to a newer block. BALs of the blocks in between carry the state
   already downloaded forward, applied strictly in block order, and the remaining ranges download at
   the new root.
4. **Repair what BALs cannot.** BALs only overwrite the fields their blocks change, so entries left
   stale for another reason are scheduled for repair. Once the BALs reach the pivot, each scheduled
   account and slot is fetched again on its own, proved against the pivot's root.
5. **Hand off.** Once every account is downloaded, the BALs reach the pivot and no repairs remain,
   the state goes to the merkle stage, which rebuilds the trie. It is accepted only when the root
   matches the pivot's header.

```rust
use reth_snap_sync::SnapPivotPolicy;

let policy = SnapPivotPolicy::default();
// Without a finalized block, anchor at the EIP's example distance.
assert_eq!(policy.pivot_block(1_000, None), Some(936));
// A recent finalized block is anchored to directly.
assert_eq!(policy.pivot_block(1_000, Some(950)), Some(950));
// Stalled finality falls back to the example distance.
assert_eq!(policy.pivot_block(1_000, Some(500)), Some(936));
// A chain shorter than the head distance has no pivot yet.
assert_eq!(policy.pivot_block(4, None), None);
```

## Design choices

- **State is written in place.** Downloads land directly in the hashed state tables, so there is no
  staging copy to move afterwards. An attempt record owns that state: every write carries the
  attempt and pivot it was fetched for, and responses to an older attempt or pivot are refused.
- **Progress commits with its data.** Each range, storage chunk or BAL commits in the same
  transaction as the progress it advances, so a stopped sync resumes from what the database holds.
- **Nothing counts before its dependencies.** An account range commits only with storage matching
  its accounts' roots and code matching their hashes. Storage too large for one response is kept
  ahead of its range, tied to the attempt and pivot.
- **BALs never skip a block.** A block whose BAL no peer serves holds back every later one, and a
  BAL only applies on top of its parent.
- **The trie is rebuilt once, at the end.** The existing merkle stage builds it from the downloaded
  state, instead of maintaining one during the download.
- **Starting over is the fallback.** An attempt restarts when catch-up falls behind the BALs peers
  still serve, or when a reorg across its pivot cannot be repaired.

## Reorgs across the pivot

A reorg that orphans the pivot leaves the downloaded state holding the abandoned branch's changes.
The attempt keeps the headers of the last 64 blocks through its pivot, so it can find the last block
both branches share and fetch the orphaned blocks' BALs:

- Every field and storage slot those BALs changed is scheduled for repair, catch-up rewinds to the
  shared block, and the attempt re-anchors to a new canonical pivot.
- Applying the new branch's BALs drops whatever they overwrite, so only entries changed on the
  orphaned branch alone stay scheduled.
- Once catch-up reaches the new pivot, those entries are fetched again on their own, proved against
  its root, and the hand-off waits until none remain.

Orphaned BALs no peer serves are waited for while the head is within the served-state window (128
blocks) of the ancestor. A reorg reaching further back than the kept headers, orphaned BALs still
unserved past that window, or a reorg after the hand-off to the merkle stage starts the attempt over
instead.

snap/1 synchronization is not covered: this design keeps the state current with BALs, which only
snap/2 serves, rather than with snap/1's trie-node healing.
