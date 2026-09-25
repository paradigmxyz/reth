# Complete state trie database PoC

`state-trie-db` switches forward Engine API payload validation to complete MDBX
tries. Account and storage reads, proof workers, sparse trie updates, in-memory
overlays, and state persistence use the new tables. The engine does not start
the hashed-post-state task or populate the legacy hashed/trie updates.

## Migration and execution

Stop the node before migration. The migration asserts that the Finish and
partial-state-trie frontiers agree before opening a write environment. It leaves
the source tables intact.

```sh
cargo +stable build --profile profiling -p reth-trie-db --example migrate_state_trie
target/profiling/examples/migrate_state_trie /schelk/reth/db --check
target/profiling/examples/migrate_state_trie /schelk/reth/db
cargo +stable build --profile profiling -p reth --features state-trie-db
target/profiling/reth node --datadir /schelk/reth
```

The migration streams sorted hashed leaves with bounded buffers. Storage roots
feed the account trie builder; all nodes are emitted as their parent paths become
known. Storage tries are currently built sequentially. The final state root must
match the existing trie. A completion marker is written only after all nodes are
committed. `--check` is read-only and also verifies the persisted root when that
marker exists. Run it before executing new blocks, while the source tables still
describe the same state.

An interrupted migration leaves partially populated destination tables.
`--restart` clears only these two tables and starts over; it refuses to clear a
completed migration. Put progress logs on a volume with sufficient free space.

This PoC supports forward payload validation with the sparse state-root task.
Serial fallback, reorgs, historical state, pipeline sync, RPC state queries,
payload building, and whole-account storage wipes are outside its scope. After
executing blocks with this feature, the legacy hashed/trie tables are stale.

## RocksDB backend

`state-trie-rocksdb` includes `state-trie-db` and selects RocksDB for the complete
trie tables. The update types, overlay merging, sparse trie and proof calculator
are shared with the MDBX backend. Execution still uses the BundleState overlay;
only database misses and proof cursors select the new backend.

```sh
cargo +stable build --profile profiling -p reth-trie-db --example migrate_state_trie
target/profiling/examples/migrate_state_trie /schelk/reth/db --rocksdb /schelk/reth/rocksdb
target/profiling/examples/migrate_state_trie /schelk/reth/db --rocksdb /schelk/reth/rocksdb --check
cargo +stable build --profile profiling -p reth-bb --features state-trie-rocksdb
```

The two column families are named `StateTrieAccounts` and `StateTrieStorages`.
Account keys use the existing 33-byte packed path. Storage keys concatenate the
32-byte account hash and the packed path, for a 65-byte key. Values use the same
tagged node encoding as MDBX. RocksDB uses the provider's existing compression,
cache, compaction and synchronous WAL settings.

Migration rebuilds from the old hashed tables, checks the legacy root, flushes
and compacts RocksDB, verifies its persisted root, and records a separate
completion marker. It retains both MDBX schemas. `--restart` applies only to the
selected destination. RocksDB size output reports SST and memtable bytes;
entry counts from RocksDB properties are estimates.

Normal commits apply the masked update batch to RocksDB before committing the
MDBX persistence frontier. Acquiring an MDBX read transaction and its RocksDB
snapshot is synchronized with that commit, so each reader sees matching
frontiers and nodes. Existing readers retain their snapshots across commits.
As with the MDBX PoC, crash recovery and reorg handling are outside the supported
forward-validation path. After replay, recover the baseline before switching
backends; each replay updates only the selected complete-trie backend.

## Tables and proofs

`StateTrieAccounts` maps packed paths to `StateTrieNode<TrieAccount>`.
`StateTrieStorages` is a duplicate-sorted table keyed by hashed account address,
with a packed path subkey and `StateTrieNode<U256>` value. Paths use 32 bytes of
zero-padded packed nibbles followed by a one-byte nibble count.

The node encoding starts with a tag and a one-byte short-key length. Tag 0 stores
an RLP-encoded leaf value. Tag 1 stores a big-endian 16-bit state mask followed by
the occupied children's RLP references, each preceded by its one-byte length.
Extensions are folded into branch nodes. Leaves use their full 64-nibble path;
branches use their logical branch path.

`proof_v3` locates the target or its nearest useful neighbor, then derives every
parent path from the short-key length and fetches it by exact key. It returns the
existing decoded proof format, including absence proofs and known-parent bounds.
No intermediate nodes are inferred from other leaves during that traversal.

Complete sparse-trie updates include leaves, branches, and deletion tombstones.
They are independent of compact trie update tracking. Overlays merge newest
values first; persistence applies the same disjoint suffix masking used for the
legacy trie updates. Execution reads use the BundleState-based execution overlay,
with direct leaf lookups in the new tables on a miss. They do not construct the
trie overlay. Proof workers also skip that overlay when the reused sparse trie
covers both persistence frontiers through the parent.

## Size measurement

The migration prints entry counts and allocated bytes for each source and
destination table. Allocated bytes are MDBX page size multiplied by the sum of
leaf, branch, and overflow pages. The reported difference compares the new two
tables with `HashedAccounts`, `HashedStorages`, `AccountsTrie`, and `StoragesTrie`.
Because migration retains those source tables, the additional database allocation
is the entire new-table total, not just the difference between schemas. MDBX file
growth and free pages can make filesystem allocation differ from table totals.

### Mainnet snapshot result

Migrated `/schelk/reth` at block **24,979,000**. Both the streaming rebuild and a
read-only `proof_v3` lookup of the persisted root matched
`0xc60084882e9a560a2679b26dbec4a5c7a6f5ac27a02985776706b82e45d7e254`.

| Table | Entries | Allocated bytes |
| --- | ---: | ---: |
| `HashedAccounts` | 382,731,390 | 31,602,118,656 |
| `HashedStorages` | 1,560,520,935 | 136,201,420,800 |
| `AccountsTrie` | 28,710,966 | 8,873,631,744 |
| `StoragesTrie` | 137,848,570 | 31,374,675,968 |
| `StateTrieAccounts` | 524,519,952 | 141,357,359,104 |
| `StateTrieStorages` | 2,094,157,438 | 367,823,278,080 |

The existing four tables total **208.05 GB (193.76 GiB)**. The new two tables
total **509.18 GB (474.21 GiB)**: **301.13 GB (280.45 GiB) more**, an increase of
**144.74%**, or **2.447×** the original table allocation.

The source tables were retained. Actual `mdbx.dat` filesystem allocation grew
from 253,988,278,272 to 744,557,142,016 bytes: **490.57 GB added**. This is less
than the new-table total because MDBX reused existing free pages. Build outputs
and the temporary replay copy are excluded from these measurements.

The completed migration wrote **2,618,677,390 nodes** in approximately 94 minutes
on dev-brian. The source contains 382,731,390 account leaves and 1,560,520,935
storage leaves; the new tables additionally contain 141,788,562 account branches
and 533,636,503 storage branches.

### Forward payload validation

On a disposable copy of the migrated datadir, the feature-enabled profiling
binary validated normal blocks **24,979,001–24,979,250** with persistence
threshold 50, memory buffer 10, and 30 state-masking blocks. All 250 returned
`VALID`. A clean shutdown brought both persistence frontiers to 24,979,250.
After restarting with persistence threshold, memory buffer, and masking all set
to zero, the next **10 blocks** also returned `VALID` while waiting for
persistence on every submission. The final Finish checkpoint was 24,979,260
with no partial-state-trie lag. The input was
`/schelk/bench/txgen-normal-mainnet-24979001-20000.ndjson`.

The original `/schelk/reth` remains at **24,979,000**, with the verified migrated
root, ready for further benchmarks. The temporary replay datadir was removed.

Validation also passed 4,230 default-feature workspace tests (37 skipped), 297
trie/engine library tests in the new mode excluding the unsupported reorg test,
50 overlay tests in the new mode, and the complete-node persistence and sparse
trie update tests. Nightly formatting, all-feature workspace Clippy, `zepter`,
TOML linting, and documentation builds passed. The workspace and feature engine
runs each had one test pass on retry.
