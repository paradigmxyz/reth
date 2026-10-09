# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.9.8](https://github.com/alloy-rs/trie/releases/tag/v0.9.8) - 2026-09-29

### Bug Fixes

- [ci] Reuse the shared deny workflow

### Dependencies

- Bump gh-actions to d5402286 ([#169](https://github.com/alloy-rs/trie/issues/169))
- [ci] Bump secure-runner pins ([#163](https://github.com/alloy-rs/trie/issues/163))

### Features

- Add TrieAccount::new and with_extension ([#170](https://github.com/alloy-rs/trie/issues/170))

### Miscellaneous Tasks

- [ci] Migrate deny to gh-actions ([#168](https://github.com/alloy-rs/trie/issues/168))
- Update gh-actions and deny workflow pins ([#167](https://github.com/alloy-rs/trie/issues/167))
- Update gh-actions to ee2960a6 ([#166](https://github.com/alloy-rs/trie/issues/166))
- [ci] Scan GitHub Actions workflows ([#165](https://github.com/alloy-rs/trie/issues/165))
- [ci] Update gh-actions pins ([#164](https://github.com/alloy-rs/trie/issues/164))
- [ci] Restore cargo-codspeed installer ([#161](https://github.com/alloy-rs/trie/issues/161))
- [ci] Route package installs through Aegis ([#158](https://github.com/alloy-rs/trie/issues/158))

## [0.9.7](https://github.com/alloy-rs/trie/releases/tag/v0.9.7) - 2026-09-21

### Bug Fixes

- [proof] Reject truncated exclusion proofs ([#134](https://github.com/alloy-rs/trie/issues/134))

### Miscellaneous Tasks

- Release 0.9.7

## [0.9.6](https://github.com/alloy-rs/trie/releases/tag/v0.9.6) - 2026-09-21

### Bug Fixes

- Preserve changelog history during releases ([#157](https://github.com/alloy-rs/trie/issues/157))
- Quote dependabot schedule time ([#148](https://github.com/alloy-rs/trie/issues/148))
- [clippy] Use sort_unstable_by_key instead of sort_unstable_by ([#122](https://github.com/alloy-rs/trie/issues/122))

### Dependencies

- [deps] Bump the ci-weekly group with 2 updates ([#159](https://github.com/alloy-rs/trie/issues/159))
- [deps] Bump CodSpeedHQ/action from 5.0.3 to 5.2.1 in the ci-weekly group ([#155](https://github.com/alloy-rs/trie/issues/155))
- [deps] Bump rui314/setup-mold from 9c9c13bf4c3f1adef0cc596abc155580bcb04444 to 7e4f20ad28a2e8ca6fd0892ccf72e2abb706b9c3 in the ci-weekly group ([#154](https://github.com/alloy-rs/trie/issues/154))
- [deps] Bump the ci-weekly group with 3 updates ([#153](https://github.com/alloy-rs/trie/issues/153))
- [deps] Bump CodSpeedHQ/action from 3.8.1 to 5.0.2 in the ci-weekly group ([#152](https://github.com/alloy-rs/trie/issues/152))
- [deps] Bump actions/checkout from 7.0.0 to 7.0.1 in the ci-weekly group ([#151](https://github.com/alloy-rs/trie/issues/151))
- [deps] Bump actions/checkout from 6.0.2 to 7.0.0 in the ci-weekly group ([#149](https://github.com/alloy-rs/trie/issues/149))
- [deps] Bump taiki-e/install-action from 2.74.0 to 2.75.26 ([#140](https://github.com/alloy-rs/trie/issues/140))
- [deps] Bump rui314/setup-mold from 725a8794d15fc7563f59595bd9556495c0564878 to 9c9c13bf4c3f1adef0cc596abc155580bcb04444 ([#139](https://github.com/alloy-rs/trie/issues/139))
- [deps] Bump CodSpeedHQ/action from 5322369bbf5359f5719bd337a19b4bcf4781fe3a to 76578c2a7ddd928664caa737f0e962e3085d4e7c ([#138](https://github.com/alloy-rs/trie/issues/138))
- [deps] Bumps ([#129](https://github.com/alloy-rs/trie/issues/129))

### Features

- Support extended trie accounts ([#156](https://github.com/alloy-rs/trie/issues/156))
- Add TrieMask::len ([#130](https://github.com/alloy-rs/trie/issues/130))
- Add TrieMask::iter_set_bits for efficient bit iteration ([#126](https://github.com/alloy-rs/trie/issues/126))

### Miscellaneous Tasks

- Release 0.9.6
- Group weekly dependabot updates ([#147](https://github.com/alloy-rs/trie/issues/147))
- Release 0.9.5 ([#136](https://github.com/alloy-rs/trie/issues/136))
- Release 0.9.4 ([#131](https://github.com/alloy-rs/trie/issues/131))
- Use thiserror for Error implementations ([#127](https://github.com/alloy-rs/trie/issues/127))
- Update CODEOWNERS ([#128](https://github.com/alloy-rs/trie/issues/128))

### Other

- Set rust-toolchain inputs explicitly ([#150](https://github.com/alloy-rs/trie/issues/150))
- Harden supply chain — pin actions, lock permissions ([#137](https://github.com/alloy-rs/trie/issues/137))
- Update to tempoxyz ([#120](https://github.com/alloy-rs/trie/issues/120))

### Performance

- Rewrite RlpNode internals with manual u8 length + MaybeUninit buffer ([#133](https://github.com/alloy-rs/trie/issues/133))

## [0.9.3](https://github.com/alloy-rs/trie/releases/tag/v0.9.3) - 2026-01-07

### Features

- Add bit ops to TrieMask ([#117](https://github.com/alloy-rs/trie/issues/117))

### Miscellaneous Tasks

- Release 0.9.3

## [0.9.2](https://github.com/alloy-rs/trie/releases/tag/v0.9.2) - 2025-12-22

### Features

- Add ordered_trie_root_encoded for pre-encoded items ([#115](https://github.com/alloy-rs/trie/issues/115))

### Miscellaneous Tasks

- Release 0.9.2
- Re-use alloy-primitives keccak empty ([#113](https://github.com/alloy-rs/trie/issues/113))
- `missing-const-for-fn` lint back to "warn". ([#112](https://github.com/alloy-rs/trie/issues/112))

## [0.9.1](https://github.com/alloy-rs/trie/releases/tag/v0.9.1) - 2025-08-19

### Documentation

- Update Tera documentation link to the correct Introduction section ([#107](https://github.com/alloy-rs/trie/issues/107))

### Features

- Retain proofs of non-target nodes in certain edge-cases. ([#109](https://github.com/alloy-rs/trie/issues/109))

### Miscellaneous Tasks

- Release 0.9.1

## [0.9.0](https://github.com/alloy-rs/trie/releases/tag/v0.9.0) - 2025-06-20

### Dependencies

- Bump nybbles to 0.4 ([#104](https://github.com/alloy-rs/trie/issues/104))
- Bump MSRV and edition ([#106](https://github.com/alloy-rs/trie/issues/106))

### Features

- Logs for pushing nodes to the stack ([#101](https://github.com/alloy-rs/trie/issues/101))

### Miscellaneous Tasks

- Release 0.9.0 ([#108](https://github.com/alloy-rs/trie/issues/108))

### Other

- More realistic nibble lengths ([#105](https://github.com/alloy-rs/trie/issues/105))
- Remove concurrency from bench ([#102](https://github.com/alloy-rs/trie/issues/102))

## [0.8.1](https://github.com/alloy-rs/trie/releases/tag/v0.8.1) - 2025-04-14

### Bug Fixes

- [verify] Inlined trie leaves ([#97](https://github.com/alloy-rs/trie/issues/97))

### Dependencies

- Bump derive more ([#98](https://github.com/alloy-rs/trie/issues/98))

### Miscellaneous Tasks

- Release 0.8.1
- [benches] Use criterion codspeed compat ([#96](https://github.com/alloy-rs/trie/issues/96))

## [0.8.0](https://github.com/alloy-rs/trie/releases/tag/v0.8.0) - 2025-04-09

### Features

- Add DecodedProofRetainer ([#84](https://github.com/alloy-rs/trie/issues/84))

### Miscellaneous Tasks

- Release 0.8.0
- Add codspeed and profiling profile ([#94](https://github.com/alloy-rs/trie/issues/94))
- Primitives 1.0 ([#92](https://github.com/alloy-rs/trie/issues/92))
- Make clippy happy ([#95](https://github.com/alloy-rs/trie/issues/95))
- Simplify word_rlp ([#87](https://github.com/alloy-rs/trie/issues/87))
- Rename groups to state_masks in hash builder ([#86](https://github.com/alloy-rs/trie/issues/86))

### Other

- Expose Serde support for `ProofNodes` and `DecodedProofNodes` ([#85](https://github.com/alloy-rs/trie/issues/85))

### Performance

- Arc `BranchNodeCompact::hashes` ([#88](https://github.com/alloy-rs/trie/issues/88))

## [0.7.9](https://github.com/alloy-rs/trie/releases/tag/v0.7.9) - 2025-02-07

### Features

- Add DecodedProofNodes struct ([#81](https://github.com/alloy-rs/trie/issues/81))

### Miscellaneous Tasks

- Release 0.7.9 ([#83](https://github.com/alloy-rs/trie/issues/83))

## [0.7.8](https://github.com/alloy-rs/trie/releases/tag/v0.7.8) - 2024-12-31

### Dependencies

- Bump and use nybbles 0.3.3 raw APIs ([#80](https://github.com/alloy-rs/trie/issues/80))

### Miscellaneous Tasks

- Release 0.7.8
- Add tests for `TrieAccount` ([#73](https://github.com/alloy-rs/trie/issues/73))

## [0.7.7](https://github.com/alloy-rs/trie/releases/tag/v0.7.7) - 2024-12-22

### Features

- Bump nybbles, use local encode_path ([#76](https://github.com/alloy-rs/trie/issues/76))

### Miscellaneous Tasks

- Release 0.7.7
- Use-single-account-buf ([#78](https://github.com/alloy-rs/trie/issues/78))

### Other

- Move deny to ci ([#75](https://github.com/alloy-rs/trie/issues/75))

## [0.7.6](https://github.com/alloy-rs/trie/releases/tag/v0.7.6) - 2024-12-04

### Features

- Add storage root fns ([#74](https://github.com/alloy-rs/trie/issues/74))

### Miscellaneous Tasks

- Release 0.7.6

## [0.7.5](https://github.com/alloy-rs/trie/releases/tag/v0.7.5) - 2024-12-04

### Dependencies

- Bump MSRV to 1.81 ([#66](https://github.com/alloy-rs/trie/issues/66))

### Documentation

- Clarify the documentation for `hash_mask` field of a branch node ([#70](https://github.com/alloy-rs/trie/issues/70))

### Features

- Migrate trie account type and state root functions from alloy ([#65](https://github.com/alloy-rs/trie/issues/65))
- `HashBuilder::add_leaf_unchecked` ([#64](https://github.com/alloy-rs/trie/issues/64))
- Derive `Clone` for `HashBuilder` ([#72](https://github.com/alloy-rs/trie/issues/72))

### Miscellaneous Tasks

- Release 0.7.5
- Add clippy settings to `Cargo.toml` ([#71](https://github.com/alloy-rs/trie/issues/71))
- Update cargo deny ([#69](https://github.com/alloy-rs/trie/issues/69))

## [0.7.4](https://github.com/alloy-rs/trie/releases/tag/v0.7.4) - 2024-11-13

### Features

- Impl Extend for ProofNodes ([#63](https://github.com/alloy-rs/trie/issues/63))

### Miscellaneous Tasks

- Release 0.7.4

## [0.7.3](https://github.com/alloy-rs/trie/releases/tag/v0.7.3) - 2024-11-07

### Documentation

- [nodes] Adjust comments about branch node masks ([#61](https://github.com/alloy-rs/trie/issues/61))

### Features

- [nodes] Make `BranchNodeRef::children` public ([#62](https://github.com/alloy-rs/trie/issues/62))

### Miscellaneous Tasks

- Release 0.7.3
- [hash-builder] Use `RlpNode::as_hash` ([#59](https://github.com/alloy-rs/trie/issues/59))

### Styling

- Migrated functions for computing trie root from reth to alloy ([#55](https://github.com/alloy-rs/trie/issues/55))

## [0.7.2](https://github.com/alloy-rs/trie/releases/tag/v0.7.2) - 2024-10-16

### Features

- [mask] Unset bit, count set bits, index of the first set bit ([#58](https://github.com/alloy-rs/trie/issues/58))
- `RlpNode::as_hash` ([#57](https://github.com/alloy-rs/trie/issues/57))

### Miscellaneous Tasks

- Release 0.7.2

## [0.7.1](https://github.com/alloy-rs/trie/releases/tag/v0.7.1) - 2024-10-14

### Bug Fixes

- Use vector of arbitrary length for leaf node value ([#56](https://github.com/alloy-rs/trie/issues/56))

### Miscellaneous Tasks

- Release 0.7.1
- Allow `Zlib` in `deny.toml` ([#54](https://github.com/alloy-rs/trie/issues/54))

## [0.7.0](https://github.com/alloy-rs/trie/releases/tag/v0.7.0) - 2024-10-14

### Bug Fixes

- Arbitrary impls ([#52](https://github.com/alloy-rs/trie/issues/52))

### Miscellaneous Tasks

- Release 0.7.0
- [meta] Add CODEOWNERS ([#47](https://github.com/alloy-rs/trie/issues/47))

### Performance

- Avoid cloning HashBuilder input ([#50](https://github.com/alloy-rs/trie/issues/50))
- Store RLP-encoded nodes using ArrayVec ([#51](https://github.com/alloy-rs/trie/issues/51))
- Avoid calculating branch node children if possible ([#49](https://github.com/alloy-rs/trie/issues/49))
- Inline RLP encode functions ([#46](https://github.com/alloy-rs/trie/issues/46))

## [0.6.0](https://github.com/alloy-rs/trie/releases/tag/v0.6.0) - 2024-09-26

### Features

- Replace std/hashbrown with alloy_primitives::map ([#42](https://github.com/alloy-rs/trie/issues/42))
- Empty root node ([#36](https://github.com/alloy-rs/trie/issues/36))

### Miscellaneous Tasks

- Release 0.6.0
- Display more information on assertions ([#40](https://github.com/alloy-rs/trie/issues/40))
- Expose `rlp_node` ([#38](https://github.com/alloy-rs/trie/issues/38))
- Remove children hashes methods ([#35](https://github.com/alloy-rs/trie/issues/35))

### Performance

- Change proof internal repr to `HashMap` ([#43](https://github.com/alloy-rs/trie/issues/43))
- [proof] Compare slices for first node ([#37](https://github.com/alloy-rs/trie/issues/37))

## [0.5.3](https://github.com/alloy-rs/trie/releases/tag/v0.5.3) - 2024-09-17

### Dependencies

- Bump msrv to 1.79 ([#33](https://github.com/alloy-rs/trie/issues/33))

### Miscellaneous Tasks

- Release 0.5.3
- Release 0.5.2
- Use `decode_raw` from `alloy-rlp` ([#19](https://github.com/alloy-rs/trie/issues/19))

### Testing

- Zero value leaf ([#34](https://github.com/alloy-rs/trie/issues/34))

## [0.5.1](https://github.com/alloy-rs/trie/releases/tag/v0.5.1) - 2024-09-02

### Bug Fixes

- No-std compat ([#31](https://github.com/alloy-rs/trie/issues/31))

### Features

- Workflow to validate no_std compatibility ([#32](https://github.com/alloy-rs/trie/issues/32))

### Miscellaneous Tasks

- Release 0.5.1

## [0.5.0](https://github.com/alloy-rs/trie/releases/tag/v0.5.0) - 2024-08-28

### Bug Fixes

- In-place nodes ignored in proof verification ([#27](https://github.com/alloy-rs/trie/issues/27))

### Dependencies

- Bump derive more ([#30](https://github.com/alloy-rs/trie/issues/30))
- [deps] Bump alloy ([#28](https://github.com/alloy-rs/trie/issues/28))
- Bump proptest ([#18](https://github.com/alloy-rs/trie/issues/18))

### Documentation

- Small fix on HashBuilderValue  docs ([#20](https://github.com/alloy-rs/trie/issues/20))

### Features

- Iterator over branch children ([#21](https://github.com/alloy-rs/trie/issues/21))

### Miscellaneous Tasks

- Release 0.5.0
- Make clippy happy ([#29](https://github.com/alloy-rs/trie/issues/29))
- Make `TrieNode` cloneable ([#22](https://github.com/alloy-rs/trie/issues/22))
- Make clippy happy ([#17](https://github.com/alloy-rs/trie/issues/17))
- Sync cliff.toml

## [0.4.1](https://github.com/alloy-rs/trie/releases/tag/v0.4.1) - 2024-05-22

### Bug Fixes

- Proofs for divergent leaf nodes ([#16](https://github.com/alloy-rs/trie/issues/16))

### Dependencies

- Move path encoding from `nybbles` ([#14](https://github.com/alloy-rs/trie/issues/14))

### Miscellaneous Tasks

- Release 0.4.1

## [0.4.0](https://github.com/alloy-rs/trie/releases/tag/v0.4.0) - 2024-05-14

### Features

- Proof verification ([#13](https://github.com/alloy-rs/trie/issues/13))
- Branch node decoding ([#12](https://github.com/alloy-rs/trie/issues/12))
- Extension node decoding ([#11](https://github.com/alloy-rs/trie/issues/11))
- Leaf node decoding ([#10](https://github.com/alloy-rs/trie/issues/10))

### Miscellaneous Tasks

- Release 0.4.0

## [0.3.1](https://github.com/alloy-rs/trie/releases/tag/v0.3.1) - 2024-04-03

### Dependencies

- Bump alloy-primitives 0.7.0 ([#8](https://github.com/alloy-rs/trie/issues/8))

### Miscellaneous Tasks

- Release 0.3.1
- Fix loop span ([#6](https://github.com/alloy-rs/trie/issues/6))

## [0.3.0](https://github.com/alloy-rs/trie/releases/tag/v0.3.0) - 2024-02-26

### Dependencies

- [deps] Bump nybbles to 0.2 ([#4](https://github.com/alloy-rs/trie/issues/4))

### Miscellaneous Tasks

- Release 0.3.0
- Clippy ([#5](https://github.com/alloy-rs/trie/issues/5))

## [0.2.1](https://github.com/alloy-rs/trie/releases/tag/v0.2.1) - 2024-01-24

### Features

- Support no_std ([#2](https://github.com/alloy-rs/trie/issues/2))

### Miscellaneous Tasks

- Release 0.2.1
- Add cliff.toml and scripts
- Simplify no_std

## [0.2.0](https://github.com/alloy-rs/trie/releases/tag/v0.2.0) - 2024-01-10

### Dependencies

- [deps] Bump alloy-primitives

### Miscellaneous Tasks

- Release 0.2.0
- Update authors ([#3](https://github.com/alloy-rs/trie/issues/3))

## [0.1.0](https://github.com/alloy-rs/trie/releases/tag/v0.1.0) - 2023-12-20

### Bug Fixes

- Miri
- Deny

### Dependencies

- Clean up dependencies

### Features

- Initial implementation extracted from reth

### Miscellaneous Tasks

- Update configs
- Increase visibility
- Remove unused module
- Clippy, docs, rm unused file
- [meta] Ci, licenses, configs

### Other

- Prealloc children ([#1](https://github.com/alloy-rs/trie/issues/1))
- Initial commit

<!-- generated by git-cliff -->
