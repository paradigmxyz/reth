# Experimental Monad TrieDB adapter

This opt-in Linux/x86-64 experiment stores `PlainAccountState`,
`PlainStorageState`, `HashedAccounts`, and `HashedStorages` in Category Labs'
native TrieDB. It is not a new implementation of TrieDB and is not a replacement
for all MDBX tables: Reth still computes and validates Ethereum state roots, and
keeps its authenticated-trie nodes, history, and chain metadata in MDBX.

The native library is pinned to
[`category-labs/monad@fd0faba2537e18a57266c0dad84c31acb08e37fa`](https://github.com/category-labs/monad/tree/fd0faba2537e18a57266c0dad84c31acb08e37fa).
It uses Monad's compressed physical trie, copy-on-write versioned roots,
compaction, io_uring, and direct block-device storage. This adapter uses a
non-hashing state machine because Reth performs the authenticated hashing.

## Build

Use the compiler and dependency requirements from the pinned Monad README
(including Clang 19+, CMake 3.27+, Boost 1.83, and recursive submodules).
Reserve 2 MiB huge pages for Monad's registered I/O buffers and permit locked
memory before starting the node or native tests. For a disposable two-node box,
`sudo sysctl -w vm.nr_hugepages=512` reserves 1 GiB; run the process with
`ulimit -l unlimited`. Keep the same reservation for baseline measurements.

```sh
git clone https://github.com/category-labs/monad.git monad
git -C monad checkout fd0faba2537e18a57266c0dad84c31acb08e37fa
git -C monad submodule update --init --recursive
cmake -S crates/storage/db/native/triedb -B target/monad-native -G Ninja \
  -DMONAD_SOURCE="$PWD/monad" -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_C_COMPILER=clang-19 -DCMAKE_CXX_COMPILER=clang++-19
cmake --build target/monad-native --target reth_monad_triedb -j
export RETH_TRIEDB_LIBRARY="$PWD/target/monad-native/libreth_monad_triedb.so"
cargo test -p reth-db --features monad-triedb native_triedb_mdbx_equivalence -- --ignored
```

Build the consuming node with the `reth-db/monad-triedb` feature enabled. Loading
the adapter requires the matching shared library and its system dependencies.
The native library and bridge are GPL-3.0-or-later; they are not covered by
Reth's MIT/Apache license. This feature is disabled by default.

## Device selection

Use a disposable database and a dedicated, unmounted device or partition. Do not
use an OS disk, mounted filesystem, or device containing data to retain.

```sh
export RETH_TRIEDB_MAP='/absolute/node/datadir=/dev/disk/by-id/DEDICATED-PARTITION'
export RETH_TRIEDB_INITIALIZE=1
```

**Initialization discards every byte on the configured device.** The mapping
uses path-component prefixes, not substring matching. Separate simultaneous
nodes require separate devices or partitions. The explicit initialization flag
is required only when the MDBX database has not been migrated; remove it after
initialization. Without a matching mapping, an ordinary database stays on MDBX.
Existing migrated databases reopen the recorded device and refuse to open if it
is missing. Binaries from this branch built without the feature also refuse to
open migrated databases; older binaries do not know this marker and must never
be used on them. Regular files are supported for tests and are created as 16 GiB
sparse files.

Migration copies the four tables in bounded batches and only clears their MDBX
contents after the native writes succeed. MDBX records the selected native
version. Subsequent transactions stage changes privately, commit the immutable
native version and synchronize its device before committing the MDBX version
pointer. A failed or aborted MDBX commit leaves an unreferenced native root;
readers continue using the version selected by their MDBX snapshot.

## Limitations

- This is a benchmark prototype, not a production storage migration tool.
- Fixed-width keys preserve MDBX ordering and duplicate-value semantics, but
  they are not Monad's production Ethereum state encoding.
- Ordered cursor seeks serialize through a native writer-side traversal; only
  exact reads use the concurrent `RODb` service. Entry counts are versioned with
  the records, so startup and stage progress queries do not scan the full state.
- One million native versions are retained; active-reader-aware retention and
  disk-pressure behavior need further work before long-running deployments.
- Upstream native invariants can terminate the process on corrupt storage or
  failed I/O. The adapter is not a corruption-recovery layer.
- Tests cover migration, cursor equivalence, aborts, snapshots, reopen, and
  unreferenced native commits, not exhaustive power-loss fault injection.
- Backups and restores must include the matched MDBX database and native device.
  An MDBX-only snapshot of a migrated database is not a complete backup.
- There is no reverse migration. Retain an original MDBX snapshot until the
  experiment and its correctness checks have completed.

Do not interpret a result from this adapter as a measurement of Monad's complete
execution architecture or of a storage backend that also replaces Reth's
authenticated-trie computation.
