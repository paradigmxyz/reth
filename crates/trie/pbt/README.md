# PBT adapters for Reth

The PBT implementation is an external dependency:
[`pbt`](https://github.com/Rimeeeeee/pbtree/tree/ba0c12d5399f327ac19328bf3b55b56d73b38154),
pinned to `ba0c12d5399f327ac19328bf3b55b56d73b38154` in the workspace `Cargo.toml`.
It is a standalone root-level PBT crate, with source, tests and tooling arranged
like [alloy-rs/trie](https://github.com/alloy-rs/trie).

This crate re-exports the external tree/state/proof, canonical artifact, and BAL
replay APIs. It keeps only the Reth-specific adapters:

- `execution.rs`: apply authoritative Reth `BundleState` output with plain keys.
- `conversion.rs`: merge Reth hashed-state cursors/code readers with preimages.
- `persistence.rs`: publish native trie checkpoints and block references using
  `reth_fs_util`, handle legacy checkpoint files, and authorize explicit pruning.

Native content-addressed trie nodes, hashing, state embedding, proofs, artifact
encoding/verification, replay, independent EELS vectors and their core tests live
in the external repository. There is no second in-tree PBT implementation.

Node lifecycle, genesis/fork selection, staged backfill and proof RPC remain in
[`reth-node-pbt`](../../ethereum/pbt/README.md). The engine adapter remains behind
`reth-engine-tree/pbt`. Both commitment domains remain available through activation.

```sh
cargo test -p reth-trie-pbt --all-features
cargo test -p reth-node-pbt
```

The feature names `execution`, `migration`, `bal`, `conversion` and `persistence`
remain available to Reth callers; the generic surfaces forward to `pbt`.
Core conformance tests run in that repository with
`cargo test -p pbt --all-features`.

This extraction preserves the existing operational boundary: BLAKE3 is a draft
reference choice, conversion and cold loading still materialize complete state,
and native PBT snap sync, automatic artifact/re-anchor discovery, signed shadow
publication remain absent. Finality-driven MPT retirement is handled by the node
integration. No mainnet schedule or gas schedule is selected here.
