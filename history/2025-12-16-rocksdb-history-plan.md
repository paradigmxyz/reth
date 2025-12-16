# RocksDB History Lookups Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix RocksDB historical state lookups to avoid reading/flattening all history shards and to skip opening RocksDB transactions when MDBX is used, matching the semantics and efficiency of the MDBX path.

**Architecture:** Keep the existing EitherReader/EitherWriter abstraction.

1) **Correctness + performance (lookups):** For history lookups, do a *single-shard seek* (the first shard with `highest_block_number >= target`), then compute the answer from that shard using the same `rank/select` logic as `history_info`. Handle the one tricky edge case (`rank == 0` + shard-first-entry > target) by checking whether a previous shard for the same key exists (via a cheap “seek first shard” comparison).

2) **Crash consistency (commits + visibility):** Copilot’s “commit RocksDB after MDBX” feedback is not a valid blanket rule for reth. The reth invariant (also used for static files) is:
   - MDBX stage checkpoints / canonical tables are the source of truth.
   - Secondary stores may be *ahead* after a crash, because recovery can truncate/prune/ignore down to the MDBX checkpoint.
   - Forward progress and unwind have different “safe” commit orders.

   The **valid** part of the review feedback is that RocksDB writes currently commit *outside* the MDBX transaction boundary, so a crash can leave RocksDB containing entries that the canonical MDBX state never committed. Because reads do **not** fall back to MDBX when routing is enabled, we must prevent “observing” those ahead entries.

   The consistency model we want is:
   - **Forward progress:** it’s OK for RocksDB to commit first, as long as MDBX only “exposes” that data after the rest of canonical state is committed.
   - **Unwind:** we must not commit RocksDB *deletes* before the MDBX unwind is committed, otherwise RocksDB can become “behind” the MDBX source of truth.

   The simplest way to make this safe and reversible is to gate RocksDB reads by an MDBX-derived **watermark** (high-water mark) so entries beyond the committed canonical range are treated as nonexistent. Optional pruning can then be done after-the-fact for space, but is not required for correctness.

3) **Micro-perf:** Gate RocksDB transaction creation on the corresponding storage setting to avoid unnecessary RocksDB transactions when the reader will route to MDBX.

**Tech Stack:** Rust, reth-provider crate, RocksDB feature, MDBX, cargo test.

### Task 1: Replace RocksDB “read all shards” with single-shard seek

**Files:**
- Modify: `crates/storage/provider/src/either_writer.rs`
- Modify: `crates/storage/provider/src/providers/state/historical.rs`

**Steps:**
1) Add `EitherReader` helpers that return **one** history shard (not `Vec`): a “seek shard at/after block” method for `AccountsHistory` and `StoragesHistory`, plus a “seek first shard” method (block 0) to support the edge-case check.
2) Rewrite the RocksDB-specific history lookup path in `historical.rs` to use the new “seek shard” method instead of `read_*_history_shards` and remove any `Vec<u64>` flattening.
3) Ensure correctness matches `history_info` for the `rank == 0` case:
   - If the shard returned by `seek(block)` has first-entry > block and **no previous shard exists**, return `NotYetWritten` unless pruned (then `InChangeset(first-entry)`).
   - If a previous shard exists, return `InChangeset(first-entry)` (the gap-between-shards case).
4) Add unit tests in the `historical.rs` test module for:
   - “gap between shards” case (requires >2000 indices so two shards are created).
   - “before first write” case (no previous shard; returns `NotYetWritten` when not pruned).
   - “no change at/after block” case (returns `InPlainState`).
   - Keep these deterministic; avoid time-based assertions.

### Task 2: Decide and document commit-order consistency model (RocksDB vs MDBX)

**Files:**
- Modify: `crates/storage/provider/src/providers/database/provider.rs`
- Modify: `crates/storage/provider/src/providers/rocksdb/provider.rs`
- Modify: `crates/node/builder/src/launch/common.rs`

**Steps:**
0) Current PR gaps (to fix): RocksDB batches are committed inline in helpers (`insert_block`, `remove_blocks_above`), so the effective order is forward **rocks → static_files → mdbx** and unwind **rocks → mdbx → static_files**. This violates the required unwind ordering (`mdbx → rocks → static_files`) and must be centralized in `DBProvider::commit()`.
1) Add a short module-level comment (or doc comment near the RocksDB routing) stating the intended invariants:
   - MDBX stage checkpoints are the source of truth.
   - RocksDB tables in this PR are derived indices and may temporarily be ahead after a crash.
   - Crash recovery can reconcile by pruning/truncating/ignoring RocksDB to the MDBX-derived watermark if needed.
2) Define **MDBX-derived watermarks** (avoid introducing new metadata if an existing committed source-of-truth already exists):
   - For `TransactionHashNumbers` in RocksDB:
     - Watermark = highest committed `TxNumber` in MDBX (derive from `tables::TransactionBlocks` or `tables::BlockBodyIndices`).
     - Reads must treat any RocksDB entry whose `tx_num > watermark` as nonexistent.
     - Writes may commit to RocksDB early; they’re only “visible” once MDBX has committed the corresponding canonical tx tables (which implies the watermark advanced).
   - For `AccountsHistory` / `StoragesHistory` in RocksDB:
     - Watermark = `StageId::IndexAccountHistory` / `StageId::IndexStorageHistory` checkpoint block in MDBX.
     - History read algorithms must ignore candidate blocks > watermark (including when an ahead append extends the last shard past the checkpoint).
   - Concretely implement the gating in the call sites that have access to MDBX state:
     - `DatabaseProvider::transaction_id`: after reading `tx_num` from RocksDB, compare against the MDBX watermark and return `Ok(None)` if it’s ahead.
     - `HistoricalStateProviderRef` RocksDB path in `historical.rs`: cap any computed “next changeset block” to `<= checkpoint` (if the first candidate is > checkpoint, treat as `InPlainState`).
3) Fix the unwind hazard for RocksDB-routed tables:
   - Do not commit RocksDB deletes for `TransactionHashNumbers` (and history shards) before the MDBX unwind checkpoint is committed.
   - Choose one of:
     - **Minimal + safe:** during unwind paths, don’t delete RocksDB synchronously; rely on the watermark to hide stale entries, and (optionally) prune RocksDB after commit for space.
    - **More invasive:** accumulate RocksDB batches and commit them from `DBProvider::commit()` in an order that matches the static-file model (forward: RocksDB before MDBX; unwind: MDBX before RocksDB).
4) Add a `ProviderFactory`-time consistency check analogous to `static_file_provider().check_consistency(...)` (optional if watermark gating is complete, but useful for disk hygiene):
   - For history tables: trim shards above the history stage checkpoint (and handle the “last shard partially ahead” case by truncating the list to `<= checkpoint`).
   - For `TransactionHashNumbers`: avoid full scans; prefer pruning based on known unwind ranges (tx hashes by range) or a bounded iterator strategy. If the table is massively ahead, consider dropping/rebuilding the CF as a recovery path.
5) Ensure the chosen approach maintains the invariant (MDBX never “exposes” data past what it has committed; RocksDB never becomes “behind” MDBX for routed reads).

**Commit sequencing we will implement (matches user expectation and static-file precedent):**
- Forward (append): `static_files.commit()` → `rocks.commit()` → `mdbx.commit()` (MDBX last; checkpoints advance only after secondary stores are durable).
- Unwind: `mdbx.commit()` → `rocks.commit()` → `static_files.commit()` (MDBX first; watermark moves back before secondary pruning).
- Action: move RocksDB batch commits out of helpers like `insert_block`/`remove_blocks_above` and centralize them in `DBProvider::commit()` so they can follow this ordering.
- Implementation approach (no watermarks):
  - Change `RocksDBBatch` to own an `Arc<RocksDBProvider>` so it can outlive stack scopes.
  - Add `DatabaseProvider` field (cfg rocksdb) `pending_rocks_batches: Vec<WriteBatchWithTransaction<true>>`.
  - At write sites, replace `batch.commit()` with `pending_rocks_batches.push(batch.into_inner())`.
  - Extend `RocksDBProvider` with `commit_batches(Vec<...>)` to flush queued batches in order.
  - In `DBProvider::commit()`, flush in the required order:
    - Forward: `static_file_provider.commit()?; rocksdb_provider.commit_batches(pending)?; tx.commit()?`
    - Unwind: `tx.commit()?; rocksdb_provider.commit_batches(pending)?; static_file_provider.commit()?`
  - On drop without commit, drop the queue (no partial RocksDB writes). This enforces ordering without watermark gating.

**Note (already done):** Feature gating simplification landed — the stub now exposes `batch()`/`tx()` and the type aliases in `either_writer.rs` use the real Rocks types, so `cargo check` passes for both default and `--features rocksdb`. No additional action needed here.

### Task 3: Avoid unnecessary RocksDB transactions for transaction lookups

**Files:**
- Modify: `crates/storage/provider/src/providers/database/provider.rs`

**Steps:**
1) In `TransactionsProvider::transaction_id`, only open a RocksDB transaction when `transaction_hash_numbers_in_rocksdb` is enabled; otherwise keep the lightweight unit placeholder so MDBX-only nodes don’t pay RocksDB snapshot cost.
2) Add a small unit test in the provider tests (cfg rocksdb) that toggles the setting off and asserts the MDBX path still returns the value; use existing test utils to write via MDBX and ensure no panic when RocksDB is unused.

### Task 4: End-to-end verification

**Files:**
- Tests only

**Steps:**
1) Run `cargo test -p reth-provider --features rocksdb -- --test-threads=1` to cover RocksDB and MDBX code paths without ENOMEM flakiness.
2) If time permits, run the reduced MDBX suite `cargo test -p reth-provider -- --test-threads=1` to confirm non-rocksdb builds still pass.

### Task 5: Cleanup and review

**Files:**
- None (repo hygiene)

**Steps:**
1) `git status` to confirm only intended files are changed.
2) Summarize changes against issue #20388 with focus on the main performance concern and the RocksDB transaction gating.
3) Hand the diff for review.
