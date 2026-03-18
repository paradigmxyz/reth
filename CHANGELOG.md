# Changelog

All notable Mantle-specific changes to this fork of [reth](https://github.com/paradigmxyz/reth) are documented here.

> **Scope:** Only Mantle-layer changes are recorded. Upstream reth commits are summarized as a single
> `rebase` entry per version. For the full upstream history see the
> [reth releases page](https://github.com/paradigmxyz/reth/releases).

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

---

## [Unreleased] — mantle-arsia

> Based on upstream reth `v1.9.3` · mantle-xyz/revm `branch: main` ⚠️

### Fixed
- fix: several incremental stability fixes (4 commits: `d136a96`, `cad01c2`, `edd83d3`, `7e14936`)

### Notes
- ⚠️ `mantle-xyz/revm` is currently pinned to `branch = "main"` rather than a fixed tag —
  reproducibility risk; should be pinned before a stable release.
- ⚠️ Mantle **Sepolia Arsia** activates **2026-03-22** (4 days from today); ensure nodes are deployed.
- Mantle **mainnet Arsia** timestamp is not yet set (`mantle_arsia_time: None`).

---

## [v2.2.0-beta.10] — 2026-03-18

### Debug
- debug: add more logs for post-execution validation diagnostics

---

## [v2.2.0-beta.9] — 2026-03-18

### Debug
- debug: add `validate_block_post_execution` warn log

---

## [v2.2.0-beta.8] — 2026-03-18

### Features
- feat: add `CREATE` contract support in `eth_estimateTotalFee` RPC

---

## [v2.2.0-beta.7] — 2026-03-17

### Features
- feat(op): align Mantle Arsia semantics (EVM / chainspec alignment)

### Fixed
- fix(op): satisfy lint implied bounds after Arsia trait additions

---

## [v2.2.0-beta.6] — 2026-03-13

### Features
- feat(op-reth): add **Mantle Sepolia** chainspec support
- feat(op-reth): add hashed snapshot import guards (prevents re-import of already-imported state)

---

## [v2.2.0-beta.5] — 2026-03-05

### Fixed
- fix(op-reth): align pre-Arsia Mantle basefee validation — skip EIP-1559 parent basefee
  check during Skadi and Limb hardfork windows; normal validation resumes at Arsia

---

## [v2.2.0-beta.4] — 2026-03-03

### Hardforks
- feat(mantle): **Limb hardfork** timestamps locked in:
  - Mainnet: `1_768_374_000` (2026-01-14 15:00 CST)
  - Sepolia: `1_764_745_200` (2025-12-03 15:00 CST)

### Features
- feat(mantle): `chainspec` — add `mantle_arsia_time` field; Arsia `BaseFeeParams(8, 2)`
- feat(mantle): receipt `token_ratio` field aligned with geth behavior

---

## [v2.2.0-beta.3] — 2026-02-25

### Features
- feat(reth): **`eth_estimateTotalFee`** RPC — Mantle-specific total fee estimation
- feat(reth): raw `eth_estimateGas` passthrough path
- feat(reth): **state-export** debug feature (`--features state-export`) — exports full state
  on state root mismatch (dev/troubleshooting only, not for production)
- feat(reth): receipt build fixes and alignment with upstream `OpReceipt`

---

## [v2.2.0-beta.1] — 2026-01-22

### Dependencies
- chore: bump mantle-xyz/revm → `tag: v2.2.0-beta.1`

---

## [v2.2.0-beta] — 2026-01-16

> Upstream reth remains `v1.9.3`.

### Hardforks
- feat: **Arsia hardfork** initial support (Sepolia-first rollout)
  - Arsia timestamp for Sepolia: `1_774_422_000` (2026-03-22 15:00 CST)
  - Mainnet timestamp: TBD

### Features
- feat: `eth_estimateGas` — Mantle gas estimation logic
- feat: add reth node metrics for Mantle
- feat: compatible with `tokenRatio` field in geth receipt format
- feat: mark `eth_getBlockRange` RPC as **deprecated**
- feat: compatible with zero hashes for non-existent accounts in `eth_getProof` (geth parity)
- feat: compatible with `eth_feeHistory` response format in geth
- feat: compatible with `eth_fillTransaction` request handling in geth
- feat: compatible with `genesis.base_fee_params` format in geth

### Fixed
- fix: receipt field alignment with geth response format

### Dependencies
- chore: bump mantle-xyz/revm → `tag: v2.2.0-beta`

---

## [v2.1.0] — 2025-12-12

### Hardforks
- feat: **Limb hardfork** scaffolding added (Osaka-equivalent, timestamps TBD at this point)

### Dependencies
- rebase: upstream reth `v1.3.12` → **`v1.9.3`** (major upstream sync, ~1800 commits)
  - Notable upstream additions: Flashblocks support, sparse-parallel trie, `rpc-convert` crate,
    `tracing-otlp`, Fusaka/Jovian hardfork stubs, performance improvements across trie and RPC
- chore: bump mantle-xyz/revm `v2.0.0` → `v2.1.0` → **`v2.1.2`**

---

## [v2.0.5] — 2025-11-27

> Based on upstream reth `v1.3.12` · mantle-xyz/revm `tag: v2.0.0`

### Hardforks
- feat: add **Mantle mainnet hardfork** schedule (Skadi activation timestamps) (#14)
  - Mainnet Skadi: `1_756_278_000` (2025-08-27 15:00 CST)
  - Sepolia Skadi: `1_752_649_200` (2025-07-16 15:00 CST)

---

## [v2.0.4] — 2025-11-10

### Fixed
- fix: preconf (pre-confirmation) error return path (#13)

---

## [v2.0.3] — 2025-10-30

### Features
- feat: `eth_suggestPriorityFee` — Optimism-compatible suggested priority fee (#8)
- feat: `eth_feeHistory` — Optimism-compatible fee history support (#10)
- feat: add two new Mantle RPC endpoints (#11)

### Changed
- opt: rename Mantle extension module for clarity (`mantle_ext`) (#12)

### Build
- chore: update DockerfileOp runtime image to `ubuntu:24.04` (#9)

---

## [v2.0.2] — 2025-10-14

### Features
- feat: support Mantle state snapshot import (`--import` command integration) (#7)

---

## [v2.0.1] — 2025-09-23

### Features
- feat: add `safe` and `finalized` block tag metrics to Mantle node (#6)

---

## [v2.0.0] — 2025-09-17

> Initial Mantle fork. Based on upstream reth **`v1.3.12`** · mantle-xyz/revm `tag: v2.0.0`

### Hardforks
- feat: **Skadi hardfork** — Mantle's Prague-equivalent upgrade; initial Mantle OP-stack
  compatibility layer

### Features
- Initial `mantle-hardforks` crate with `MantleHardfork` enum
- Mantle mainnet chainspec (`crates/optimism/chainspec/src/mantle.rs`,
  `mantle_mainnet.rs`)
- `MantleEthApiExtServer` — Mantle-specific RPC extension trait
- `OpBeaconConsensus` extended with `MantleHardforks` bound
- `DockerfileOp` for containerized op-reth builds

---

[Unreleased]: https://github.com/mantle-xyz/reth/compare/v2.2.0-beta.10...HEAD
[v2.2.0-beta.10]: https://github.com/mantle-xyz/reth/compare/v2.2.0-beta.9...v2.2.0-beta.10
[v2.2.0-beta.9]: https://github.com/mantle-xyz/reth/compare/v2.2.0-beta.8...v2.2.0-beta.9
[v2.2.0-beta.8]: https://github.com/mantle-xyz/reth/compare/v2.2.0-beta.7...v2.2.0-beta.8
[v2.2.0-beta.7]: https://github.com/mantle-xyz/reth/compare/v2.2.0-beta.6...v2.2.0-beta.7
[v2.2.0-beta.6]: https://github.com/mantle-xyz/reth/compare/v2.2.0-beta.5...v2.2.0-beta.6
[v2.2.0-beta.5]: https://github.com/mantle-xyz/reth/compare/v2.2.0-beta.4...v2.2.0-beta.5
[v2.2.0-beta.4]: https://github.com/mantle-xyz/reth/compare/v2.2.0-beta.3...v2.2.0-beta.4
[v2.2.0-beta.3]: https://github.com/mantle-xyz/reth/compare/v2.2.0-beta.1...v2.2.0-beta.3
[v2.2.0-beta.1]: https://github.com/mantle-xyz/reth/compare/v2.2.0-beta...v2.2.0-beta.1
[v2.2.0-beta]: https://github.com/mantle-xyz/reth/compare/v2.1.0...v2.2.0-beta
[v2.1.0]: https://github.com/mantle-xyz/reth/compare/v2.0.5...v2.1.0
[v2.0.5]: https://github.com/mantle-xyz/reth/compare/v2.0.4...v2.0.5
[v2.0.4]: https://github.com/mantle-xyz/reth/compare/v2.0.3...v2.0.4
[v2.0.3]: https://github.com/mantle-xyz/reth/compare/v2.0.2...v2.0.3
[v2.0.2]: https://github.com/mantle-xyz/reth/compare/v2.0.1...v2.0.2
[v2.0.1]: https://github.com/mantle-xyz/reth/compare/v2.0.0...v2.0.1
[v2.0.0]: https://github.com/mantle-xyz/reth/releases/tag/v2.0.0
