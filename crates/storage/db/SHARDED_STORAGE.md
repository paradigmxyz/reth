# Experimental prefix-sharded persistence

The parallel-table experiment leaves hashed storage and the storage trie on one
worker each. Split each logical table into four physical DUPSORT tables by the
top two bits of the storage-slot hash / first trie nibble. This distributes a
single hot contract, unlike address-only sharding. Account state/trie stay intact.

The original table name is shard zero; suffixes `Shard1` through `Shard3` hold the
remaining ranges. Keys, values, trie encoding, state roots, and logical ordering
are unchanged. Legacy and packed trie encodings have different byte shifts but
the same nibble partition. Empty trie paths are not persisted, as before.

## Invariants

- Exactly one worker owns each physical-table cursor. Workers never use the parent
  transaction. All workers join and drop cursors before children merge.
- Children are merged serially; **one parent commit** makes all tables and block
  metadata visible atomically. This does not introduce independent durable commits.
- Logical cursors merge `(key, encoded value)` ordering across shards. Duplicate
  seeks start in the matching prefix range; deletion of all duplicates visits all
  shards. `get`, `entries`, `clear`, and raw typed views use the logical table.
- Logical cursors must explicitly re-seek after another cursor mutates the same
  table, and after a failed movement before relying on their position. Their
  ordered-value anchors do not reproduce all native MDBX cursor bookkeeping:
  deleting another cursor's row and inserting before its former successor can
  change that cursor's next result. Arbitrary interleaved mutation-position
  equivalence is not supported. Persistence workers use exclusive physical
  cursors and do not depend on this logical-cursor behavior.
- Exact hashed-storage point reads open only their selected physical shard, then
  verify subkey equality. The restricted cursor must not be used for range scans.
- Schema version 3 rejects old unsharded databases. New `init` and binary-dump
  imports naturally populate the new tables through normal database APIs.
- The conversion helper exclusively locks an offline database. It reads the trie
  encoding from persisted storage settings, fences normal opens with version
  3000003, and repartitions both tables plus a completion marker in one transaction.
  Both version-file transitions use fsync + atomic rename. An interrupted conversion
  is resumed by rerunning it; never manually replace the version file.

## Tempo comparison

`tempo bench-shard-storage --database DATADIR/db` invokes the conversion helper.
The accompanying `scripts/bench-prepare-storage-layout.sh` and `bench-e2e.nu` hook
restore the same baseline-generated state before each phase. For an unsharded
baseline versus a sharded candidate, `scripts/bench-cache-storage-layout.sh`
converts an independent copy once, outside measurement. Both layouts are kept in
the virgin snapshot under separate paths: the ordinary `db/` remains unsharded.
After each restore, only the candidate activates the pristine converted copy.
Ownership, job-key, schema, and available-space checks fail closed; an interrupted
conversion never replaces the source database. Other comparisons retain the
per-phase conversion path. Baseline snapshot generation uses the baseline binary.

The harness syncs and drops page caches after preparation on both sides so the
conversion does not grant a warm-cache advantage. Compare paired runs and an
identical-baseline control, keep warmup/workload/OTEL settings equal, and report
persistence latency and backpressure alongside TPS. Migration time is not included
in node throughput. Caching requires space for both layouts and migration scratch
pages; it trades extra disk usage for avoiding repeated large conversions.

This remains an experimental MDBX fork, not a production-compatible upgrade.
Cursor/reopen/abort/migration recovery tests and successful benchmarks do not prove
race freedom, power-loss safety, or correctness under arbitrary allocation failures.
