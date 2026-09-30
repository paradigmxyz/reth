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
   whether supplied with it or stored beforehand.
3. **Advance the pivot.** Peers only keep recent state, so once the pivot lags more than 96 blocks
   by default the sync re-anchors to a newer block. BALs of the blocks in between carry the state
   already downloaded forward, applied strictly in block order, and the remaining ranges download at
   the new root.
4. **Hand off.** Once every account is downloaded and the BALs reach the pivot, the state goes to
   the merkle stage, which rebuilds the trie. It is accepted only when the root matches the pivot's
   header.

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
  still serve.

Reorg recovery is not implemented yet: a reorg that orphans the pivot is detected and restarts the
attempt, instead of repairing the state from the orphaned blocks' BALs as EIP-8189 describes.

snap/1 synchronization is not covered: this design keeps the state current with BALs, which only
snap/2 serves, rather than with snap/1's trie-node healing.
