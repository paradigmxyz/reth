# TIP-1143 account representation dependencies

These are local forks of registry releases alloy-trie 0.9.8, revm-state 43.0.1,
and reth-primitives-traits 0.8.2. They make the code metadata type explicit across
trie, database, and execution outcomes without guessing from opaque byte prefixes.

Legacy extensions keep their bytes and encodings. Typed extensions retain opaque
fields alongside bounded code metadata. Their fifth trie element is an RLP list
`[1, opaque, code_metadata]`, while old extensions are RLP strings. Compact records
retain the existing length-delimited opaque field, then append an explicit version,
metadata length, and metadata. A valid old record ends at the opaque length and
cannot collide with this representation.

Alloy and revm share the same one-pointer allocation with an in-memory kind tag.
Conversions into EVM2 separate the opaque bytes from structured code metadata.
JSON and MessagePack encode typed extensions as objects; legacy byte encodings
remain unchanged. Typed extensions require a self-describing Serde format. Borsh
explicitly rejects the new representation instead of silently losing code metadata.

These patches define a draft local format. They do not activate a network fork or
migrate any existing account.

These source snapshots are included here so the draft can build without sibling checkouts. Package versions and upstream source commits are retained in each crate’s `.cargo_vcs_info.json`; original licenses are retained. Changes originate from integration dependency commit `755cce341ea6` and remain draft protocol formats.
