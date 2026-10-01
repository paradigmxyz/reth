# Unified RocksDB state trie experiment

On this branch, `state-trie-rocksdb` uses one column family named `StateTrie`.
`StateTrieAccounts` and `StateTrieStorages` remain available as migration sources;
normal execution, proofs, and persistence use the unified family.

Keys preserve the existing packed-path ordering:

| Record | Key |
| --- | --- |
| Account branch or leaf | 32-byte zero-padded packed path, then nibble length |
| Storage branch or leaf | 32-byte account hash, byte `64`, then 32-byte zero-padded packed storage path and nibble length |

Account keys are 33 bytes and storage keys are 66 bytes. Every account leaf key
is the prefix of its storage keys, so the account sorts immediately before its
storage subtree. The length byte distinguishes an account leaf from shorter
account branches, including branches whose padded path equals an account hash.
An empty storage path cannot collide with its account record. Values retain the
existing tagged `StateTrieNode<TrieAccount>` or `StateTrieNode<U256>` encoding.
The two Rust table types are typed views of the same column family, not separate
physical tables; generic typed iteration must not cross into the other view.

Point reads and MultiGet use the unified keys. Account neighbor cursors jump over
whole storage ranges; storage cursors stay within their account prefix. Proof v3,
sparse-trie pruning, update extraction, and overlay skipping otherwise retain the
parent branch's behavior. Persistence merges sorted account and storage updates
into one SST per update batch, including deletion tombstones, and publishes it
with the existing snapshot/checkpoint protocol.

Stop the node, recover the desired snapshot, and run:

```sh
cargo build --profile profiling -p reth-provider \
  --example migrate_unified_state_trie --features state-trie-rocksdb
target/profiling/examples/migrate_unified_state_trie DATADIR/db DATADIR/rocksdb
```

The migration first asserts `partial_state_trie == Finish`. It requires an empty
destination, streams both existing RocksDB state-trie tables in merged key order,
and ingests bounded SST files. Source keys and values are not modified. It then
compares every destination record against the sources and checks the state root
against the root reconstructed from the preserved account leaves. `--check` performs only this verification. It is
valid before executing new blocks against the unified table, while the sources
still represent the same state. An interrupted migration can be restarted after
recovering the snapshot; it does not silently overwrite a partial destination.

`--root` only prints the durable frontier and unified trie root, without comparing
source records. Replay harnesses can compare this with their known snapshot root
after recovery, avoiding another full migration verification scan.
