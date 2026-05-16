# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.7.3](https://github.com/alloy-rs/alloy/releases/tag/v1.7.3) - 2026-02-17

### Miscellaneous Tasks

- Release 1.7.2

## [1.7.1](https://github.com/alloy-rs/alloy/releases/tag/v1.7.1) - 2026-02-17

### Documentation

- Fix incomplete return condition in next_block_excess_blob_gas ([#3679](https://github.com/alloy-rs/alloy/issues/3679))

### Features

- [consensus] Add `EthereumReceipt::map_logs` ([#3710](https://github.com/alloy-rs/alloy/issues/3710))
- [consensus] Add `EthereumReceipt` type ported from reth ([#3683](https://github.com/alloy-rs/alloy/issues/3683))
- Add trait impls for Cow to support Recovered<Cow<'_, T>> ([#3689](https://github.com/alloy-rs/alloy/issues/3689))
- [consensus] Add `InMemorySize` trait ([#3656](https://github.com/alloy-rs/alloy/issues/3656))

### Miscellaneous Tasks

- Release 1.7.1
- Release 1.7.0
- Release 1.6.3
- Release 1.6.2

## [1.6.1](https://github.com/alloy-rs/alloy/releases/tag/v1.6.1) - 2026-02-03

### Miscellaneous Tasks

- Release 1.6.1

### Other

- Revert "feat(rpc-types-eth): add optional timestamp field to Transaction" ([#3638](https://github.com/alloy-rs/alloy/issues/3638))

## [1.6.0](https://github.com/alloy-rs/alloy/releases/tag/v1.6.0) - 2026-02-03

### Bug Fixes

- [consensus] Mark tx_mut as hidden with invalidation warning ([#3608](https://github.com/alloy-rs/alloy/issues/3608))

### Features

- [rpc-types-eth] Add optional timestamp field to Transaction ([#3606](https://github.com/alloy-rs/alloy/issues/3606))
- Adding an ECDSA verify ([#3595](https://github.com/alloy-rs/alloy/issues/3595))

### Miscellaneous Tasks

- Release 1.6.0

## [1.5.2](https://github.com/alloy-rs/alloy/releases/tag/v1.5.2) - 2026-01-22

### Miscellaneous Tasks

- Release 1.5.2

## [1.5.1](https://github.com/alloy-rs/alloy/releases/tag/v1.5.1) - 2026-01-21

### Miscellaneous Tasks

- Release 1.5.1 ([#3569](https://github.com/alloy-rs/alloy/issues/3569))

## [1.5.0](https://github.com/alloy-rs/alloy/releases/tag/v1.5.0) - 2026-01-21

### Features

- [envelope-macro] Generate `tx_type` ([#3563](https://github.com/alloy-rs/alloy/issues/3563))

### Miscellaneous Tasks

- Release 1.5.0

### Other

- Remove unused debug import from envelope module ([#3531](https://github.com/alloy-rs/alloy/issues/3531))

## [1.4.3](https://github.com/alloy-rs/alloy/releases/tag/v1.4.3) - 2026-01-14

### Features

- [consensus] Relax Block::decode_sealed to accept any H: Decodable ([#3523](https://github.com/alloy-rs/alloy/issues/3523))
- [consensus] Add decode_sealed for efficient sealed block/header decoding ([#3519](https://github.com/alloy-rs/alloy/issues/3519))

### Miscellaneous Tasks

- Release 1.4.3
- Release 1.4.2

## [1.4.1](https://github.com/alloy-rs/alloy/releases/tag/v1.4.1) - 2026-01-13

### Bug Fixes

- Support Eip7594 blob format for tx build ([#3446](https://github.com/alloy-rs/alloy/issues/3446))

### Miscellaneous Tasks

- Release 1.4.1
- Release 1.4.0
- Remove dead random buffer from custom envelope test ([#3456](https://github.com/alloy-rs/alloy/issues/3456))
- Update alloy-trie ([#3454](https://github.com/alloy-rs/alloy/issues/3454))
- Fix misleading comments ([#3445](https://github.com/alloy-rs/alloy/issues/3445))

## [1.3.0](https://github.com/alloy-rs/alloy/releases/tag/v1.3.0) - 2026-01-06

### Documentation

- [consensus] Correct doc comment for authorization_list ([#3442](https://github.com/alloy-rs/alloy/issues/3442))
- `s/EIP-4337/ERC-4337/g;` ([#3431](https://github.com/alloy-rs/alloy/issues/3431))

### Features

- [consensus] Add HeaderRoots type ([#3427](https://github.com/alloy-rs/alloy/issues/3427))
- [`contract`] Add sidecar_7594 to CallBuilder ([#3424](https://github.com/alloy-rs/alloy/issues/3424))

### Miscellaneous Tasks

- Release 1.3.0
- Add deprecated type alias back ([#3422](https://github.com/alloy-rs/alloy/issues/3422))

## [1.2.1](https://github.com/alloy-rs/alloy/releases/tag/v1.2.1) - 2025-12-23

### Bug Fixes

- Simplify size functions ([#3403](https://github.com/alloy-rs/alloy/issues/3403))

### Dependencies

- [deps] Run cargo shear ([#3405](https://github.com/alloy-rs/alloy/issues/3405))

### Documentation

- Fix try_into_eip7702 documentation ([#3317](https://github.com/alloy-rs/alloy/issues/3317))

### Features

- [consensus] Add buffer-based signer recovery methods ([#3340](https://github.com/alloy-rs/alloy/issues/3340))

### Miscellaneous Tasks

- Release 1.2.1
- Rm all deprecations ([#3341](https://github.com/alloy-rs/alloy/issues/3341))

## [1.1.3](https://github.com/alloy-rs/alloy/releases/tag/v1.1.3) - 2025-12-06

### Bug Fixes

- [consensus] Silence unused generic param in Recovered::try_convert ([#3274](https://github.com/alloy-rs/alloy/issues/3274))

### Documentation

- Fix incorrect max_fee_per_gas field documentation ([#3250](https://github.com/alloy-rs/alloy/issues/3250))

### Miscellaneous Tasks

- Release 1.1.3
- Prefix hash constants with 0x ([#3272](https://github.com/alloy-rs/alloy/issues/3272))

## [1.1.2](https://github.com/alloy-rs/alloy/releases/tag/v1.1.2) - 2025-11-20

### Features

- `Eip2718DecodableReceipt` ([#3225](https://github.com/alloy-rs/alloy/issues/3225))
- Add helper `TypedTransaction::decode_unsigned()` ([#3198](https://github.com/alloy-rs/alloy/issues/3198))

### Miscellaneous Tasks

- Release 1.1.2

## [1.1.1](https://github.com/alloy-rs/alloy/releases/tag/v1.1.1) - 2025-11-13

### Bug Fixes

- [consensus] Correct Option type in Header::size() method ([#3143](https://github.com/alloy-rs/alloy/issues/3143))

### Features

- Add EIP-7594 conversion and sidecar manipulation API ([#3144](https://github.com/alloy-rs/alloy/issues/3144))
- [consensus,eips,genesis] Add Borsh support ([#2946](https://github.com/alloy-rs/alloy/issues/2946))

### Miscellaneous Tasks

- Release 1.1.1

### Refactor

- [consensus] Remove Borsh skip attributes from tx structs ([#3155](https://github.com/alloy-rs/alloy/issues/3155))

## [1.1.0](https://github.com/alloy-rs/alloy/releases/tag/v1.1.0) - 2025-11-04

### Dependencies

- Bump MSRV to 1.88 ([#3123](https://github.com/alloy-rs/alloy/issues/3123))

### Features

- Add missing conversion fns ([#3124](https://github.com/alloy-rs/alloy/issues/3124))
- Add map sidecar fns ([#3122](https://github.com/alloy-rs/alloy/issues/3122))

### Miscellaneous Tasks

- Release 1.1.0

### Other

- Restrict consensus transaction tests to the serde feature ([#3130](https://github.com/alloy-rs/alloy/issues/3130))

## [1.0.42](https://github.com/alloy-rs/alloy/releases/tag/v1.0.42) - 2025-10-31

### Bug Fixes

- Correct gas field types in Header::size() method ([#3074](https://github.com/alloy-rs/alloy/issues/3074))

### Features

- [envelope] Add try_into_* consuming helpers for EthereumTxEnvelope and tests ([#3062](https://github.com/alloy-rs/alloy/issues/3062))
- Add convenience fn for TxEip4844WithSidecar 7594 conversion ([#3040](https://github.com/alloy-rs/alloy/issues/3040))

### Miscellaneous Tasks

- Release 1.0.42
- Fix clippy ([#3044](https://github.com/alloy-rs/alloy/issues/3044))
- Release 1.0.41
- Release 1.0.40

## [1.0.39](https://github.com/alloy-rs/alloy/releases/tag/v1.0.39) - 2025-10-16

### Bug Fixes

- Saturate gas_price deser for TxLegacy ([#3010](https://github.com/alloy-rs/alloy/issues/3010))

### Features

- Add helper for legacy -> 7594 sidecar conversion ([#3013](https://github.com/alloy-rs/alloy/issues/3013))

### Miscellaneous Tasks

- Release 1.0.39
- Add rlp test ([#3032](https://github.com/alloy-rs/alloy/issues/3032))

## [1.0.38](https://github.com/alloy-rs/alloy/releases/tag/v1.0.38) - 2025-10-08

### Features

- Generate TypedTransaction enum in envelope macro ([#2925](https://github.com/alloy-rs/alloy/issues/2925))

### Miscellaneous Tasks

- Release 1.0.38 ([#3007](https://github.com/alloy-rs/alloy/issues/3007))

### Refactor

- [consensus] Unify try_get_provider body across std/no_std ([#2963](https://github.com/alloy-rs/alloy/issues/2963))

## [1.0.37](https://github.com/alloy-rs/alloy/releases/tag/v1.0.37) - 2025-09-30

### Bug Fixes

- [consensus] Correct deprecated method references in Recovered<T> ([#2950](https://github.com/alloy-rs/alloy/issues/2950))
- [consensus] Replace redundant set_chain_id call with comment in set_chain_id_checked ([#2948](https://github.com/alloy-rs/alloy/issues/2948))

### Features

- Add `UnsupportedTransactionType` error ([#2928](https://github.com/alloy-rs/alloy/issues/2928))

### Miscellaneous Tasks

- Release 1.0.37
- Use fqs for tx_hash_with_type forwarding ([#2944](https://github.com/alloy-rs/alloy/issues/2944))
- Remove feature(doc_auto_cfg) ([#2941](https://github.com/alloy-rs/alloy/issues/2941))
- Fix a couple of grammatical errors ([#2931](https://github.com/alloy-rs/alloy/issues/2931))

### Testing

- Test display of `UnsupportedTransactionType` error and its conversion into `TransactionBuilderError` ([#2929](https://github.com/alloy-rs/alloy/issues/2929))

## [1.0.36](https://github.com/alloy-rs/alloy/releases/tag/v1.0.36) - 2025-09-24

### Miscellaneous Tasks

- Release 1.0.36
- Forward std optional ([#2917](https://github.com/alloy-rs/alloy/issues/2917))
- Remove redundant copy ([#2916](https://github.com/alloy-rs/alloy/issues/2916))

## [1.0.35](https://github.com/alloy-rs/alloy/releases/tag/v1.0.35) - 2025-09-22

### Bug Fixes

- Don't use serde private API ([#2909](https://github.com/alloy-rs/alloy/issues/2909))

### Miscellaneous Tasks

- Release 1.0.35

## [1.0.34](https://github.com/alloy-rs/alloy/releases/tag/v1.0.34) - 2025-09-21

### Miscellaneous Tasks

- Release 1.0.34

## [1.0.33](https://github.com/alloy-rs/alloy/releases/tag/v1.0.33) - 2025-09-19

### Miscellaneous Tasks

- Release 1.0.33

## [1.0.32](https://github.com/alloy-rs/alloy/releases/tag/v1.0.32) - 2025-09-16

### Bug Fixes

- [consensus] Include EIP-7702 in ReceiptEnvelope Arbitrary range ([#2883](https://github.com/alloy-rs/alloy/issues/2883))

### Miscellaneous Tasks

- Release 1.0.32

## [1.0.31](https://github.com/alloy-rs/alloy/releases/tag/v1.0.31) - 2025-09-15

### Miscellaneous Tasks

- Release 1.0.31

### Refactor

- Consolidate effective gas price calculation into eip1559 module ([#2872](https://github.com/alloy-rs/alloy/issues/2872))

## [1.0.30](https://github.com/alloy-rs/alloy/releases/tag/v1.0.30) - 2025-09-03

### Miscellaneous Tasks

- Release 1.0.30

## [1.0.29](https://github.com/alloy-rs/alloy/releases/tag/v1.0.29) - 2025-09-03

### Dependencies

- Revert "chore(deps): internal dep bumps" ([#2839](https://github.com/alloy-rs/alloy/issues/2839))

### Miscellaneous Tasks

- Release 1.0.29

## [1.0.28](https://github.com/alloy-rs/alloy/releases/tag/v1.0.28) - 2025-09-02

### Dependencies

- [deps] Internal dep bumps ([#2834](https://github.com/alloy-rs/alloy/issues/2834))

### Features

- Add Asref for recovered withencoded ([#2828](https://github.com/alloy-rs/alloy/issues/2828))

### Miscellaneous Tasks

- Release 1.0.28
- Add into_envelope helper ([#2832](https://github.com/alloy-rs/alloy/issues/2832))
- Use trait upcasting ([#2827](https://github.com/alloy-rs/alloy/issues/2827))

### Styling

- [Feature] Implement support different signatures in envelope macro ([#2794](https://github.com/alloy-rs/alloy/issues/2794))

## [1.0.27](https://github.com/alloy-rs/alloy/releases/tag/v1.0.27) - 2025-08-26

### Features

- Fusaka changes ([#2821](https://github.com/alloy-rs/alloy/issues/2821))
- Add TxHashRef trait and implementations ([#2751](https://github.com/alloy-rs/alloy/issues/2751))
- Add helpers for encoding block from parts ([#2809](https://github.com/alloy-rs/alloy/issues/2809))
- Expose alloy trie ([#2773](https://github.com/alloy-rs/alloy/issues/2773))

### Miscellaneous Tasks

- Release 1.0.27 ([#2822](https://github.com/alloy-rs/alloy/issues/2822))
- Release 1.0.26
- Release 1.0.25
- Fix warnings ([#2799](https://github.com/alloy-rs/alloy/issues/2799))
- Add typos ([#2787](https://github.com/alloy-rs/alloy/issues/2787))

## [1.0.24](https://github.com/alloy-rs/alloy/releases/tag/v1.0.24) - 2025-08-06

### Features

- Add headerinfo helper type ([#2766](https://github.com/alloy-rs/alloy/issues/2766))

### Miscellaneous Tasks

- Release 1.0.24

## [1.0.23](https://github.com/alloy-rs/alloy/releases/tag/v1.0.23) - 2025-07-22

### Miscellaneous Tasks

- Release 1.0.23

## [1.0.22](https://github.com/alloy-rs/alloy/releases/tag/v1.0.22) - 2025-07-14

### Miscellaneous Tasks

- Release 1.0.22

## [1.0.21](https://github.com/alloy-rs/alloy/releases/tag/v1.0.21) - 2025-07-14

### Features

- Add sidecar helpers ([#2697](https://github.com/alloy-rs/alloy/issues/2697))

### Miscellaneous Tasks

- Release 1.0.21
- Add into_with_bloom_unchecked ([#2683](https://github.com/alloy-rs/alloy/issues/2683))
- Add Recovered::copied ([#2680](https://github.com/alloy-rs/alloy/issues/2680))

### Other

- Remove redundant comment in BlockBody arbitrary implementation ([#2702](https://github.com/alloy-rs/alloy/issues/2702))
- Add EIP-2930 link and clarify access_list docs ([#2691](https://github.com/alloy-rs/alloy/issues/2691))

## [1.0.20](https://github.com/alloy-rs/alloy/releases/tag/v1.0.20) - 2025-07-09

### Miscellaneous Tasks

- Release 1.0.20

## [1.0.19](https://github.com/alloy-rs/alloy/releases/tag/v1.0.19) - 2025-07-08

### Miscellaneous Tasks

- Release 1.0.19

## [1.0.18](https://github.com/alloy-rs/alloy/releases/tag/v1.0.18) - 2025-07-08

### Features

- From_typed to envelope ([#2658](https://github.com/alloy-rs/alloy/issues/2658))
- Add dynamic crypto backend for ecrecover ([#2634](https://github.com/alloy-rs/alloy/issues/2634))
- Add dedicated error for SECP256K1N_HALF error ([#2636](https://github.com/alloy-rs/alloy/issues/2636))

### Miscellaneous Tasks

- Release 1.0.18
- Move impls to signed ([#2671](https://github.com/alloy-rs/alloy/issues/2671))
- Make cargo t compile ([#2657](https://github.com/alloy-rs/alloy/issues/2657))
- Release 1.0.17

## [1.0.16](https://github.com/alloy-rs/alloy/releases/tag/v1.0.16) - 2025-06-27

### Bug Fixes

- Encode into buf ([#2632](https://github.com/alloy-rs/alloy/issues/2632))

### Miscellaneous Tasks

- Release 1.0.16

## [1.0.15](https://github.com/alloy-rs/alloy/releases/tag/v1.0.15) - 2025-06-27

### Miscellaneous Tasks

- Release 1.0.15
- Rename ported fn ([#2629](https://github.com/alloy-rs/alloy/issues/2629))

## [1.0.14](https://github.com/alloy-rs/alloy/releases/tag/v1.0.14) - 2025-06-27

### Features

- Add recover_signer_unchecked_with_buf to `SignerRecoverable` trait ([#2626](https://github.com/alloy-rs/alloy/issues/2626))

### Miscellaneous Tasks

- Release 1.0.14

## [1.0.13](https://github.com/alloy-rs/alloy/releases/tag/v1.0.13) - 2025-06-26

### Features

- [tx-macros] Add `arbitrary_cfg` parameter ([#2616](https://github.com/alloy-rs/alloy/issues/2616))
- Add with_header method to Block type ([#2604](https://github.com/alloy-rs/alloy/issues/2604))
- Implement SignerRecoverable for Signed<T> ([#2596](https://github.com/alloy-rs/alloy/issues/2596))

### Miscellaneous Tasks

- Release 1.0.13

## [1.0.12](https://github.com/alloy-rs/alloy/releases/tag/v1.0.12) - 2025-06-18

### Features

- More serde compat for `TransactionEnvelope` macro ([#2594](https://github.com/alloy-rs/alloy/issues/2594))

### Miscellaneous Tasks

- Release 1.0.12
- Release 1.0.11

## [1.0.10](https://github.com/alloy-rs/alloy/releases/tag/v1.0.10) - 2025-06-17

### Bug Fixes

- Fix incorrect type flag doc for EIP-4844 + minor grammar in CI config ([#2554](https://github.com/alloy-rs/alloy/issues/2554))

### Dependencies

- Bump MSRV to 1.85 ([#2547](https://github.com/alloy-rs/alloy/issues/2547))

### Features

- Implement `TransactionEnvelope` derive macro ([#2585](https://github.com/alloy-rs/alloy/issues/2585))
- Default into_logs fn ([#2539](https://github.com/alloy-rs/alloy/issues/2539))
- Add TryFrom conversions for Extended ([#2520](https://github.com/alloy-rs/alloy/issues/2520))

### Miscellaneous Tasks

- Release 1.0.10
- Release 1.0.10
- Fix typo in comment [crates/consensus/src/transaction/mod.rs] ([#2569](https://github.com/alloy-rs/alloy/issues/2569))
- Ommers_hashes helper ([#2537](https://github.com/alloy-rs/alloy/issues/2537))
- Add ReceiptEnvelope helpers ([#2533](https://github.com/alloy-rs/alloy/issues/2533))

### Other

- Fix typo in comment ([#2561](https://github.com/alloy-rs/alloy/issues/2561))

### Testing

- Fix typo in test function name in receipts.rs ([#2580](https://github.com/alloy-rs/alloy/issues/2580))
- Add sanity assert trait impl test ([#2552](https://github.com/alloy-rs/alloy/issues/2552))

## [1.0.9](https://github.com/alloy-rs/alloy/releases/tag/v1.0.9) - 2025-05-28

### Miscellaneous Tasks

- Release 1.0.9

## [1.0.8](https://github.com/alloy-rs/alloy/releases/tag/v1.0.8) - 2025-05-27

### Miscellaneous Tasks

- Release 1.0.8
- Add try_into helper fns ([#2515](https://github.com/alloy-rs/alloy/issues/2515))

## [1.0.7](https://github.com/alloy-rs/alloy/releases/tag/v1.0.7) - 2025-05-24

### Features

- Add Extended type with alloy trait impls ([#2498](https://github.com/alloy-rs/alloy/issues/2498))
- Encodable2718:into_encoded ([#2486](https://github.com/alloy-rs/alloy/issues/2486))

### Miscellaneous Tasks

- Release 1.0.7

## [1.0.6](https://github.com/alloy-rs/alloy/releases/tag/v1.0.6) - 2025-05-21

### Miscellaneous Tasks

- Release 1.0.6

## [1.0.5](https://github.com/alloy-rs/alloy/releases/tag/v1.0.5) - 2025-05-20

### Miscellaneous Tasks

- Release 1.0.5

## [1.0.4](https://github.com/alloy-rs/alloy/releases/tag/v1.0.4) - 2025-05-19

### Documentation

- Fix typos and improve documentation clarity in serde-related modules ([#2475](https://github.com/alloy-rs/alloy/issues/2475))

### Features

- [consensus] Sidecar generic (round 2) ([#2466](https://github.com/alloy-rs/alloy/issues/2466))
- [consensus] `TxEip4844Variant` generic over sidecar ([#2461](https://github.com/alloy-rs/alloy/issues/2461))

### Miscellaneous Tasks

- Release 1.0.4
- Warn missing-const-for-fn ([#2418](https://github.com/alloy-rs/alloy/issues/2418))
- Rm leftover recovery impl ([#2467](https://github.com/alloy-rs/alloy/issues/2467))
- [consensus] Relax 4844 with sidecar creation ([#2465](https://github.com/alloy-rs/alloy/issues/2465))

### Other

- SignerRecoverable for WithEncoded<T> ([#2474](https://github.com/alloy-rs/alloy/issues/2474))

## [1.0.3](https://github.com/alloy-rs/alloy/releases/tag/v1.0.3) - 2025-05-15

### Bug Fixes

- [`consensus`] Allow `"accessList": null` when deserializing EIP-1559 transactions. ([#2450](https://github.com/alloy-rs/alloy/issues/2450))

### Features

- [consensus] Relax `TxEip4844WithSidecar` trait implementations ([#2446](https://github.com/alloy-rs/alloy/issues/2446))

### Miscellaneous Tasks

- Release 1.0.3 ([#2460](https://github.com/alloy-rs/alloy/issues/2460))
- Release 1.0.2
- Relax some conversions ([#2456](https://github.com/alloy-rs/alloy/issues/2456))

### Testing

- [eips] Add tests for EIP-7594 sidecar ([#2449](https://github.com/alloy-rs/alloy/issues/2449))

## [1.0.1](https://github.com/alloy-rs/alloy/releases/tag/v1.0.1) - 2025-05-13

### Miscellaneous Tasks

- Release 1.0.1

## [1.0.0](https://github.com/alloy-rs/alloy/releases/tag/v1.0.0) - 2025-05-13

### Features

- Add source to recovery err ([#2424](https://github.com/alloy-rs/alloy/issues/2424))
- [consensus] Generic sidecar for 4844 ([#2434](https://github.com/alloy-rs/alloy/issues/2434))

### Miscellaneous Tasks

- Release 1.0.0
- Remove shadowed recovery fn ([#2438](https://github.com/alloy-rs/alloy/issues/2438))

## [0.15.11](https://github.com/alloy-rs/alloy/releases/tag/v0.15.11) - 2025-05-12

### Documentation

- Update docs ([#2413](https://github.com/alloy-rs/alloy/issues/2413))

### Features

- Impl Signerrecoverable trait ([#2423](https://github.com/alloy-rs/alloy/issues/2423))

### Miscellaneous Tasks

- Release 0.15.11

### Refactor

- Improve and simplify event filters ([#2140](https://github.com/alloy-rs/alloy/issues/2140))

## [0.15.10](https://github.com/alloy-rs/alloy/releases/tag/v0.15.10) - 2025-05-07

### Miscellaneous Tasks

- Release 0.15.10

### Other

- Propagate arb feature ([#2407](https://github.com/alloy-rs/alloy/issues/2407))

### Styling

- Chore : fix typos ([#2398](https://github.com/alloy-rs/alloy/issues/2398))

## [0.15.9](https://github.com/alloy-rs/alloy/releases/tag/v0.15.9) - 2025-05-05

### Features

- Add more IsTyped2718 impls ([#2396](https://github.com/alloy-rs/alloy/issues/2396))
- Add IsTyped2718  ([#2394](https://github.com/alloy-rs/alloy/issues/2394))

### Miscellaneous Tasks

- Release 0.15.9

## [0.15.8](https://github.com/alloy-rs/alloy/releases/tag/v0.15.8) - 2025-05-02

### Features

- Support deserializing system signatures in legacy transactions ([#2358](https://github.com/alloy-rs/alloy/issues/2358))

### Miscellaneous Tasks

- Release 0.15.8

### Testing

- Make test compile ([#2377](https://github.com/alloy-rs/alloy/issues/2377))

## [0.15.7](https://github.com/alloy-rs/alloy/releases/tag/v0.15.7) - 2025-04-30

### Features

- Add bloom_ref ([#2366](https://github.com/alloy-rs/alloy/issues/2366))
- [consensus] Add `secp256k1` sender recovery ([#2352](https://github.com/alloy-rs/alloy/issues/2352))

### Miscellaneous Tasks

- Release 0.15.7
- Add bloom_ref ([#2368](https://github.com/alloy-rs/alloy/issues/2368))

### Other

- Revert "feat: add bloom_ref" ([#2367](https://github.com/alloy-rs/alloy/issues/2367))

## [0.15.6](https://github.com/alloy-rs/alloy/releases/tag/v0.15.6) - 2025-04-24

### Miscellaneous Tasks

- Release 0.15.6

## [0.15.5](https://github.com/alloy-rs/alloy/releases/tag/v0.15.5) - 2025-04-24

### Features

- Add more conversions ([#2344](https://github.com/alloy-rs/alloy/issues/2344))

### Miscellaneous Tasks

- Release 0.15.5
- Relax rpc tx conversions ([#2345](https://github.com/alloy-rs/alloy/issues/2345))
- Release 0.15.4

## [0.15.3](https://github.com/alloy-rs/alloy/releases/tag/v0.15.3) - 2025-04-24

### Features

- Add new_unchecked ([#2343](https://github.com/alloy-rs/alloy/issues/2343))

### Miscellaneous Tasks

- Release 0.15.3
- Move txtype to dedicated mod ([#2342](https://github.com/alloy-rs/alloy/issues/2342))
- Update upcasting TODOs ([#2340](https://github.com/alloy-rs/alloy/issues/2340))

## [0.15.2](https://github.com/alloy-rs/alloy/releases/tag/v0.15.2) - 2025-04-23

### Miscellaneous Tasks

- Release 0.15.2
- More 4844 conversions ([#2339](https://github.com/alloy-rs/alloy/issues/2339))

## [0.15.1](https://github.com/alloy-rs/alloy/releases/tag/v0.15.1) - 2025-04-23

### Miscellaneous Tasks

- Release 0.15.1
- More 4844 conversions ([#2338](https://github.com/alloy-rs/alloy/issues/2338))

## [0.15.0](https://github.com/alloy-rs/alloy/releases/tag/v0.15.0) - 2025-04-23

### Bug Fixes

- Fix Typos in Documentation Comments ([#2325](https://github.com/alloy-rs/alloy/issues/2325))

### Features

- Add txenvelope helpers ([#2322](https://github.com/alloy-rs/alloy/issues/2322))
- Add pooled conversion ([#2321](https://github.com/alloy-rs/alloy/issues/2321))
- Add to recovered ref fns ([#2316](https://github.com/alloy-rs/alloy/issues/2316))

### Miscellaneous Tasks

- Release 0.15.0
- Relax into typed fn ([#2323](https://github.com/alloy-rs/alloy/issues/2323))

### Styling

-  Added Is_dyanamic_fee to TxType ([#2296](https://github.com/alloy-rs/alloy/issues/2296))

## [0.14.0](https://github.com/alloy-rs/alloy/releases/tag/v0.14.0) - 2025-04-09

### Bug Fixes

- Fix docs of input field for different tx ([#2177](https://github.com/alloy-rs/alloy/issues/2177))

### Dependencies

- [deps] Core 1.0 ([#2184](https://github.com/alloy-rs/alloy/issues/2184))
- [deps] Bincode 2.0 ([#2297](https://github.com/alloy-rs/alloy/issues/2297))

### Features

- [eip4844] Implement `AsRef` and `AsMut` for `TxEip4844` ([#2272](https://github.com/alloy-rs/alloy/issues/2272))
- Add bincode compat to eth typed tx ([#2269](https://github.com/alloy-rs/alloy/issues/2269))
- [`consensus`] WithEncoded helpers ([#2266](https://github.com/alloy-rs/alloy/issues/2266))

### Miscellaneous Tasks

- Release 0.14.0
- Port transaction envelope bincode compat function ([#2263](https://github.com/alloy-rs/alloy/issues/2263))
- Hide input mut ([#2255](https://github.com/alloy-rs/alloy/issues/2255))

### Styling

- Added TxType::is_eipxxx fxns ([#2275](https://github.com/alloy-rs/alloy/issues/2275))

## [0.13.0](https://github.com/alloy-rs/alloy/releases/tag/v0.13.0) - 2025-03-28

### Features

- Add bincode compat for receipt envelope ([#2246](https://github.com/alloy-rs/alloy/issues/2246))

### Miscellaneous Tasks

- Release 0.13.0
- Expect instead of allow ([#2228](https://github.com/alloy-rs/alloy/issues/2228))
- Propagate arbitrary feature ([#2227](https://github.com/alloy-rs/alloy/issues/2227))

### Other

- Added input-mut for TxEnvelope ([#2244](https://github.com/alloy-rs/alloy/issues/2244))

### Testing

- Fix flaky test ([#2248](https://github.com/alloy-rs/alloy/issues/2248))

## [0.12.6](https://github.com/alloy-rs/alloy/releases/tag/v0.12.6) - 2025-03-18

### Dependencies

- Bump once_cell ([#2185](https://github.com/alloy-rs/alloy/issues/2185))

### Features

- [consensus] Add hoodi genesis hash ([#2210](https://github.com/alloy-rs/alloy/issues/2210))
- Derive `Serialize` and `Deserialize` for `Recovered<T>` ([#2204](https://github.com/alloy-rs/alloy/issues/2204))
- Impl into_transaction TxEnvelope ([#2192](https://github.com/alloy-rs/alloy/issues/2192))
- Add missing U8 conversion ([#2189](https://github.com/alloy-rs/alloy/issues/2189))

### Miscellaneous Tasks

- Release 0.12.6

### Other

- Add encodable and decodable for `Signed<T>` ([#2193](https://github.com/alloy-rs/alloy/issues/2193))

### Styling

- Update tx fee comment about Transaction trait ([#2208](https://github.com/alloy-rs/alloy/issues/2208))

## [0.12.5](https://github.com/alloy-rs/alloy/releases/tag/v0.12.5) - 2025-03-12

### Features

- [`consensus`] `TxEnvelope` generic over `Eip4844` ([#2169](https://github.com/alloy-rs/alloy/issues/2169))

### Miscellaneous Tasks

- Release 0.12.5
- Remove associated constant from RlpEcdsaEncodableTx ([#2172](https://github.com/alloy-rs/alloy/issues/2172))
- Impl Hash for Signed ([#2170](https://github.com/alloy-rs/alloy/issues/2170))
- Use default type for receipt ([#2168](https://github.com/alloy-rs/alloy/issues/2168))

## [0.12.4](https://github.com/alloy-rs/alloy/releases/tag/v0.12.4) - 2025-03-07

### Bug Fixes

- Use `OnceBox` ([#2165](https://github.com/alloy-rs/alloy/issues/2165))

### Miscellaneous Tasks

- Release 0.12.4

## [0.12.3](https://github.com/alloy-rs/alloy/releases/tag/v0.12.3) - 2025-03-07

### Features

- Add Eip4844 variant generic to TypedTransaction ([#2162](https://github.com/alloy-rs/alloy/issues/2162))

### Miscellaneous Tasks

- Release 0.12.3

## [0.12.2](https://github.com/alloy-rs/alloy/releases/tag/v0.12.2) - 2025-03-07

### Miscellaneous Tasks

- Release 0.12.2
- Release 0.12.1
- Add inner_mut ([#2160](https://github.com/alloy-rs/alloy/issues/2160))

## [0.12.0](https://github.com/alloy-rs/alloy/releases/tag/v0.12.0) - 2025-03-07

### Bug Fixes

- Run zepter checks for features of non-workspace dependencies ([#2144](https://github.com/alloy-rs/alloy/issues/2144))

### Dependencies

- [deps] Bump derive_more, strum ([#2074](https://github.com/alloy-rs/alloy/issues/2074))

### Features

- Use `OnceCell` for `Signed::hash` ([#2025](https://github.com/alloy-rs/alloy/issues/2025))
- Integrate `Recovered` into more types ([#2151](https://github.com/alloy-rs/alloy/issues/2151))
- Add bincode compat for receipt ([#2149](https://github.com/alloy-rs/alloy/issues/2149))
- [`consensus`] Impl RlpEncodableTx for TypedTx ([#2150](https://github.com/alloy-rs/alloy/issues/2150))
- Add conversion helper for eip658 status ([#2148](https://github.com/alloy-rs/alloy/issues/2148))
- [`consensus`] Separate RlpTx trait functionality ([#2138](https://github.com/alloy-rs/alloy/issues/2138))
- Add conversions for UnknownTxEnvelope ([#2133](https://github.com/alloy-rs/alloy/issues/2133))
- Add signabletx impl for typedtx ([#2131](https://github.com/alloy-rs/alloy/issues/2131))
- Allow getting mutable inner from receipt envelope ([#2116](https://github.com/alloy-rs/alloy/issues/2116))
- Add more transaction conversion helpers ([#2103](https://github.com/alloy-rs/alloy/issues/2103))
- Derive `Copy` for `Recovered` ([#2082](https://github.com/alloy-rs/alloy/issues/2082))
- Add function selector helper ([#2066](https://github.com/alloy-rs/alloy/issues/2066))
- [`consensus`] Introduce block traits ([#2057](https://github.com/alloy-rs/alloy/issues/2057))
- Introduce error helper and fallible conversion ([#2052](https://github.com/alloy-rs/alloy/issues/2052))

### Miscellaneous Tasks

- Release 0.12.0
- Support static error msg ([#2158](https://github.com/alloy-rs/alloy/issues/2158))
- [`consensus`] Rename `Recovered` methods ([#2155](https://github.com/alloy-rs/alloy/issues/2155))
- Box value ([#2152](https://github.com/alloy-rs/alloy/issues/2152))
- Add From<Signed<TypedTransaction>> for TxEnvelope ([#2070](https://github.com/alloy-rs/alloy/issues/2070))
- Add try_into_pooled conversion ([#2056](https://github.com/alloy-rs/alloy/issues/2056))
- Additional From TryFrom conversion helpers ([#2054](https://github.com/alloy-rs/alloy/issues/2054))

### Other

- Implement Transaction type on Either type ([#2097](https://github.com/alloy-rs/alloy/issues/2097))

## [0.11.1](https://github.com/alloy-rs/alloy/releases/tag/v0.11.1) - 2025-02-12

### Features

- Add auth count helper fn ([#2007](https://github.com/alloy-rs/alloy/issues/2007))
- Add blob_count helper fn ([#2005](https://github.com/alloy-rs/alloy/issues/2005))

### Miscellaneous Tasks

- Release 0.11.1

## [0.11.0](https://github.com/alloy-rs/alloy/releases/tag/v0.11.0) - 2025-01-31

### Documentation

- Enable some useful rustdoc features on docs.rs ([#1890](https://github.com/alloy-rs/alloy/issues/1890))

### Features

- Add blockbody ommers generic ([#1964](https://github.com/alloy-rs/alloy/issues/1964))
- Introduce maybe helpers for blob calc ([#1962](https://github.com/alloy-rs/alloy/issues/1962))
- Add some doc aliases for recovered ([#1961](https://github.com/alloy-rs/alloy/issues/1961))
- Couple convenience methods ([#1955](https://github.com/alloy-rs/alloy/issues/1955))
- Add map fns to rpc transaction type ([#1936](https://github.com/alloy-rs/alloy/issues/1936))
- Add Recovered::cloned ([#1932](https://github.com/alloy-rs/alloy/issues/1932))
- Add more derives for `Receipts` ([#1930](https://github.com/alloy-rs/alloy/issues/1930))
- [consensus] Make fn tx_type() public ([#1926](https://github.com/alloy-rs/alloy/issues/1926))
- Add rlp length helper ([#1906](https://github.com/alloy-rs/alloy/issues/1906))
- Remove T: Transport from public APIs ([#1859](https://github.com/alloy-rs/alloy/issues/1859))
- Add RecoveredTx::try_map_transaction ([#1885](https://github.com/alloy-rs/alloy/issues/1885))
- Add missing helper fns ([#1880](https://github.com/alloy-rs/alloy/issues/1880))

### Miscellaneous Tasks

- Release 0.11.0
- Use u64 for base fee in tx info ([#1963](https://github.com/alloy-rs/alloy/issues/1963))
- Dont enable serde in tests ([#1966](https://github.com/alloy-rs/alloy/issues/1966))
- Add receipt conversion fns ([#1949](https://github.com/alloy-rs/alloy/issues/1949))
- Add as_recovered_ref ([#1933](https://github.com/alloy-rs/alloy/issues/1933))
- [eips] Add super trait `Typed2718` to `Encodable2718` ([#1913](https://github.com/alloy-rs/alloy/issues/1913))
- [consensus] Replace magic numbers for tx type with constants ([#1911](https://github.com/alloy-rs/alloy/issues/1911))
- Release 0.10.0

### Other

- Add zepter and propagate features ([#1951](https://github.com/alloy-rs/alloy/issues/1951))

### Testing

- Migrate 4844 rlp tests ([#1928](https://github.com/alloy-rs/alloy/issues/1928))

## [0.9.2](https://github.com/alloy-rs/alloy/releases/tag/v0.9.2) - 2025-01-03

### Miscellaneous Tasks

- Release 0.9.2

## [0.9.1](https://github.com/alloy-rs/alloy/releases/tag/v0.9.1) - 2024-12-30

### Bug Fixes

- Use u64 for all gas values ([#1848](https://github.com/alloy-rs/alloy/issues/1848))

### Features

- Add deref for block ([#1868](https://github.com/alloy-rs/alloy/issues/1868))
- Add tryfrom payloadv1 for block ([#1851](https://github.com/alloy-rs/alloy/issues/1851))
- Add match functions ([#1847](https://github.com/alloy-rs/alloy/issues/1847))
- Add BlockConditional ([#1846](https://github.com/alloy-rs/alloy/issues/1846))
- EIP-7840 ([#1828](https://github.com/alloy-rs/alloy/issues/1828))
- Return tagged variant deserde error ([#1810](https://github.com/alloy-rs/alloy/issues/1810))
- [pectra] Revert EIP-7742 ([#1807](https://github.com/alloy-rs/alloy/issues/1807))
- Add map transactions fn ([#1827](https://github.com/alloy-rs/alloy/issues/1827))
- Add helpers for block ([#1816](https://github.com/alloy-rs/alloy/issues/1816))
- Add helpers to any tx envelope ([#1817](https://github.com/alloy-rs/alloy/issues/1817))

### Miscellaneous Tasks

- Release 0.9.1
- Add arbitrary for blockbody ([#1867](https://github.com/alloy-rs/alloy/issues/1867))
- Rm unused alloy-signer dep ([#1862](https://github.com/alloy-rs/alloy/issues/1862))
- Rm non exhaustive from ReceiptEnvelope ([#1843](https://github.com/alloy-rs/alloy/issues/1843))
- Rm non exhaustive for envelope ([#1842](https://github.com/alloy-rs/alloy/issues/1842))
- Map header fns ([#1840](https://github.com/alloy-rs/alloy/issues/1840))

### Other

- Change `chain_id` type to `U256` ([#1839](https://github.com/alloy-rs/alloy/issues/1839))

## [0.8.3](https://github.com/alloy-rs/alloy/releases/tag/v0.8.3) - 2024-12-20

### Features

- Add serde for block ([#1814](https://github.com/alloy-rs/alloy/issues/1814))

### Miscellaneous Tasks

- Release 0.8.3

## [0.8.2](https://github.com/alloy-rs/alloy/releases/tag/v0.8.2) - 2024-12-19

### Bug Fixes

- Relax legacy chain id check ([#1809](https://github.com/alloy-rs/alloy/issues/1809))

### Miscellaneous Tasks

- Release 0.8.2
- Manual default impl ([#1813](https://github.com/alloy-rs/alloy/issues/1813))
- Misc clippy ([#1812](https://github.com/alloy-rs/alloy/issues/1812))

## [0.8.1](https://github.com/alloy-rs/alloy/releases/tag/v0.8.1) - 2024-12-16

### Features

- Add some helper functions for blockbody ([#1796](https://github.com/alloy-rs/alloy/issues/1796))
- Add info tx types ([#1793](https://github.com/alloy-rs/alloy/issues/1793))
- Reth's block body fns ([#1775](https://github.com/alloy-rs/alloy/issues/1775))
- Add serde for `TxType` ([#1780](https://github.com/alloy-rs/alloy/issues/1780))

### Miscellaneous Tasks

- Release 0.8.1
- Add arbitrary for block ([#1797](https://github.com/alloy-rs/alloy/issues/1797))
- Add helpers to unwrap a variant ([#1792](https://github.com/alloy-rs/alloy/issues/1792))
- Add clone_tx ([#1791](https://github.com/alloy-rs/alloy/issues/1791))
- Add TxReceipt default helpers ([#1783](https://github.com/alloy-rs/alloy/issues/1783))
- Add consensus helper methods to BlockHeader ([#1781](https://github.com/alloy-rs/alloy/issues/1781))

## [0.8.0](https://github.com/alloy-rs/alloy/releases/tag/v0.8.0) - 2024-12-10

### Bug Fixes

- Use asref impl for receipt ([#1758](https://github.com/alloy-rs/alloy/issues/1758))

### Features

- [consensus] Require typed2718 for transaction ([#1746](https://github.com/alloy-rs/alloy/issues/1746))
- Port reth pooled tx type ([#1767](https://github.com/alloy-rs/alloy/issues/1767))

### Miscellaneous Tasks

- Release 0.8.0 ([#1778](https://github.com/alloy-rs/alloy/issues/1778))
- Introduce recovered and recoverable ([#1768](https://github.com/alloy-rs/alloy/issues/1768))

### Other

- Reapply "feat(consensus): require typed2718 for transaction ([#1746](https://github.com/alloy-rs/alloy/issues/1746))" ([#1773](https://github.com/alloy-rs/alloy/issues/1773))
- Revert "feat(consensus): require typed2718 for transaction ([#1746](https://github.com/alloy-rs/alloy/issues/1746))" ([#1772](https://github.com/alloy-rs/alloy/issues/1772))

## [0.7.3](https://github.com/alloy-rs/alloy/releases/tag/v0.7.3) - 2024-12-05

### Bug Fixes

- Adjust EIP-7742 to latest spec ([#1713](https://github.com/alloy-rs/alloy/issues/1713))
- Pass slice to RlpReceipt::rlp_decode_fields ([#1696](https://github.com/alloy-rs/alloy/issues/1696))
- [`consensus`] Serde aliases to avoid breaking changes ([#1654](https://github.com/alloy-rs/alloy/issues/1654))

### Documentation

- Fix `SignableTransaction` docs to use `PrimitiveSignature` ([#1743](https://github.com/alloy-rs/alloy/issues/1743))

### Features

- Impl Encodable / Decodable for Receipts ([#1752](https://github.com/alloy-rs/alloy/issues/1752))
- Add `BlockHeader::blob_fee` ([#1754](https://github.com/alloy-rs/alloy/issues/1754))
- Migrate to `TrieAccount` of alloy-trie ([#1750](https://github.com/alloy-rs/alloy/issues/1750))
- Move is_empty to trait function ([#1749](https://github.com/alloy-rs/alloy/issues/1749))
- Make Receipt rlp methods pub ([#1731](https://github.com/alloy-rs/alloy/issues/1731))
- Receipt root fn ([#1708](https://github.com/alloy-rs/alloy/issues/1708))
- Impl `Encodable2718` for `ReceiptWithBloom` ([#1719](https://github.com/alloy-rs/alloy/issues/1719))
- Add blob_gas_used ([#1704](https://github.com/alloy-rs/alloy/issues/1704))
- EIP-7742 ([#1600](https://github.com/alloy-rs/alloy/issues/1600))
- Add parent_num_hash to BlockHeader ([#1687](https://github.com/alloy-rs/alloy/issues/1687))
- Modifiy ReceiptWithBloom and associated impls to use with Reth ([#1672](https://github.com/alloy-rs/alloy/issues/1672))
- [consensus-tx] Enable fast `is_create` ([#1683](https://github.com/alloy-rs/alloy/issues/1683))
- Add `next_block_base_fee` to `BlockHeader` trait ([#1682](https://github.com/alloy-rs/alloy/issues/1682))
- Add missing size fn ([#1679](https://github.com/alloy-rs/alloy/issues/1679))
- Introduce Typed2718 trait ([#1675](https://github.com/alloy-rs/alloy/issues/1675))
- Move `AnyReceipt` and `AnyHeader` to `alloy-consensus-any` ([#1609](https://github.com/alloy-rs/alloy/issues/1609))
- Add missing txtype tryfroms ([#1651](https://github.com/alloy-rs/alloy/issues/1651))
- Add rlp for txtype ([#1648](https://github.com/alloy-rs/alloy/issues/1648))

### Miscellaneous Tasks

- Release 0.7.3
- Export storage root fns ([#1756](https://github.com/alloy-rs/alloy/issues/1756))
- Re-export stateroot fns ([#1753](https://github.com/alloy-rs/alloy/issues/1753))
- Rm redundant generic ([#1737](https://github.com/alloy-rs/alloy/issues/1737))
- Relax ommers root fn ([#1736](https://github.com/alloy-rs/alloy/issues/1736))
- Add missing from impl ([#1732](https://github.com/alloy-rs/alloy/issues/1732))
- Release 0.7.2 ([#1729](https://github.com/alloy-rs/alloy/issues/1729))
- Release 0.7.0
- Relax from impl ([#1698](https://github.com/alloy-rs/alloy/issues/1698))
- Make clippy happy ([#1677](https://github.com/alloy-rs/alloy/issues/1677))
- Export typed2718 ([#1678](https://github.com/alloy-rs/alloy/issues/1678))
- Add default for txtype ([#1668](https://github.com/alloy-rs/alloy/issues/1668))
- Add num hash with parent ([#1652](https://github.com/alloy-rs/alloy/issues/1652))
- Add some proof fns ([#1645](https://github.com/alloy-rs/alloy/issues/1645))
- Add transactions iter fn ([#1646](https://github.com/alloy-rs/alloy/issues/1646))
- Add partialEq to txtype ([#1647](https://github.com/alloy-rs/alloy/issues/1647))

### Other

- Add blanket impl of Transaction, TxReceipt and BlockHeader references ([#1657](https://github.com/alloy-rs/alloy/issues/1657))
- Add unit tests for tx envelope ([#1656](https://github.com/alloy-rs/alloy/issues/1656))
- Inline getters in impl of `Transaction` ([#1642](https://github.com/alloy-rs/alloy/issues/1642))

## [0.6.4](https://github.com/alloy-rs/alloy/releases/tag/v0.6.4) - 2024-11-12

### Bug Fixes

- Make EIP-155 signatures logic safer ([#1641](https://github.com/alloy-rs/alloy/issues/1641))

### Miscellaneous Tasks

- Release 0.6.4

### Other

- Add trait method `Transaction::effective_gas_price` ([#1640](https://github.com/alloy-rs/alloy/issues/1640))

## [0.6.3](https://github.com/alloy-rs/alloy/releases/tag/v0.6.3) - 2024-11-12

### Bug Fixes

- Serde for transactions ([#1630](https://github.com/alloy-rs/alloy/issues/1630))

### Features

- [consensus] `TxEnvelope::signature` ([#1634](https://github.com/alloy-rs/alloy/issues/1634))

### Miscellaneous Tasks

- Release 0.6.3
- Release 0.6.2 ([#1632](https://github.com/alloy-rs/alloy/issues/1632))

### Other

- Add trait method `Transaction::is_dynamic_fee` ([#1638](https://github.com/alloy-rs/alloy/issues/1638))

## [0.6.1](https://github.com/alloy-rs/alloy/releases/tag/v0.6.1) - 2024-11-06

### Bug Fixes

- Re-introduce HeaderResponse trait ([#1627](https://github.com/alloy-rs/alloy/issues/1627))

### Miscellaneous Tasks

- Release 0.6.1

## [0.6.0](https://github.com/alloy-rs/alloy/releases/tag/v0.6.0) - 2024-11-06

### Bug Fixes

- Serde for `AnyTxEnvelope` ([#1613](https://github.com/alloy-rs/alloy/issues/1613))
- Receipt status serde ([#1608](https://github.com/alloy-rs/alloy/issues/1608))
- Hash handling ([#1604](https://github.com/alloy-rs/alloy/issues/1604))
- RLP for `TxEip4844` ([#1596](https://github.com/alloy-rs/alloy/issues/1596))
- Add more rlp correctness checks ([#1595](https://github.com/alloy-rs/alloy/issues/1595))
- Clearer replay protection checks ([#1581](https://github.com/alloy-rs/alloy/issues/1581))
- Make a sensible encoding api ([#1496](https://github.com/alloy-rs/alloy/issues/1496))

### Features

- Integrate signature with boolean parity ([#1540](https://github.com/alloy-rs/alloy/issues/1540))
- Implement Arbitrary for transaction types ([#1603](https://github.com/alloy-rs/alloy/issues/1603))
- Add impl From<Header> for AnyHeader ([#1592](https://github.com/alloy-rs/alloy/issues/1592))
- [consensus] Protected Legacy Signature ([#1578](https://github.com/alloy-rs/alloy/issues/1578))
- Embed consensus header into RPC ([#1573](https://github.com/alloy-rs/alloy/issues/1573))

### Miscellaneous Tasks

- Release 0.6.0
- Misc clippy ([#1607](https://github.com/alloy-rs/alloy/issues/1607))
- Add blockbody default ([#1559](https://github.com/alloy-rs/alloy/issues/1559))

### Other

- Rm useless `len` var in `rlp_encoded_fields_length` ([#1612](https://github.com/alloy-rs/alloy/issues/1612))
- Rm `Receipts` `root_slow` unused method ([#1567](https://github.com/alloy-rs/alloy/issues/1567))
- Embed TxEnvelope into `rpc-types-eth::Transaction` ([#1460](https://github.com/alloy-rs/alloy/issues/1460))
- Implement `root_slow` for `Receipts` ([#1563](https://github.com/alloy-rs/alloy/issues/1563))
- Add `uncle_block_from_header` impl and test ([#1554](https://github.com/alloy-rs/alloy/issues/1554))
- Fix `HOLESKY_GENESIS_HASH` ([#1555](https://github.com/alloy-rs/alloy/issues/1555))

## [0.5.4](https://github.com/alloy-rs/alloy/releases/tag/v0.5.4) - 2024-10-23

### Miscellaneous Tasks

- Release 0.5.4

## [0.5.3](https://github.com/alloy-rs/alloy/releases/tag/v0.5.3) - 2024-10-22

### Bug Fixes

- Correct implementations of Encodable and Decodable for sidecars ([#1528](https://github.com/alloy-rs/alloy/issues/1528))
- Maybetagged serde for typed transaction ([#1495](https://github.com/alloy-rs/alloy/issues/1495))

### Miscellaneous Tasks

- Release 0.5.3

### Other

- Add `Debug` trait bound for `Transaction` trait ([#1543](https://github.com/alloy-rs/alloy/issues/1543))
- Use `Withdrawals` wrapper in `BlockBody` ([#1525](https://github.com/alloy-rs/alloy/issues/1525))

## [0.5.2](https://github.com/alloy-rs/alloy/releases/tag/v0.5.2) - 2024-10-18

### Bug Fixes

- Fix requests root ([#1521](https://github.com/alloy-rs/alloy/issues/1521))
- Use Decodable directly ([#1522](https://github.com/alloy-rs/alloy/issues/1522))

### Miscellaneous Tasks

- Release 0.5.2
- Make Header encoding good ([#1524](https://github.com/alloy-rs/alloy/issues/1524))
- Reorder bincode modules ([#1520](https://github.com/alloy-rs/alloy/issues/1520))

### Testing

- Extend test with rlp ([#1523](https://github.com/alloy-rs/alloy/issues/1523))

## [0.5.1](https://github.com/alloy-rs/alloy/releases/tag/v0.5.1) - 2024-10-18

### Miscellaneous Tasks

- Release 0.5.1
- Remove 7685 request variants ([#1515](https://github.com/alloy-rs/alloy/issues/1515))

## [0.5.0](https://github.com/alloy-rs/alloy/releases/tag/v0.5.0) - 2024-10-18

### Bug Fixes

- [`rpc-types-eth`] Receipt deser ([#1506](https://github.com/alloy-rs/alloy/issues/1506))
- Use `requests_hash` ([#1508](https://github.com/alloy-rs/alloy/issues/1508))
- Allow missing-tag deser of tx envelope ([#1489](https://github.com/alloy-rs/alloy/issues/1489))
- Rename gas_limit to gas in serde def for txns ([#1486](https://github.com/alloy-rs/alloy/issues/1486))
- Enforce correct parity for legacy transactions ([#1428](https://github.com/alloy-rs/alloy/issues/1428))

### Features

- From impl for variant ([#1488](https://github.com/alloy-rs/alloy/issues/1488))
- `Encodable2718::network_len` ([#1431](https://github.com/alloy-rs/alloy/issues/1431))

### Miscellaneous Tasks

- Release 0.5.0
- Flatten eip-7685 requests into a single opaque list ([#1383](https://github.com/alloy-rs/alloy/issues/1383))
- Rename requests root to requests hash ([#1379](https://github.com/alloy-rs/alloy/issues/1379))
- [consensus] Test use Vec::with_capacity ([#1476](https://github.com/alloy-rs/alloy/issues/1476))
- Some lifetime simplifications ([#1467](https://github.com/alloy-rs/alloy/issues/1467))
- Some small improvements ([#1461](https://github.com/alloy-rs/alloy/issues/1461))
- Apply same member order ([#1408](https://github.com/alloy-rs/alloy/issues/1408))

### Other

- Rm redundant root hash definitions ([#1501](https://github.com/alloy-rs/alloy/issues/1501))
- Add more constraints to `TxReceipt` trait ([#1478](https://github.com/alloy-rs/alloy/issues/1478))
- Replace `to` by `kind` in Transaction trait ([#1484](https://github.com/alloy-rs/alloy/issues/1484))

### Refactor

- Change input output to Bytes ([#1487](https://github.com/alloy-rs/alloy/issues/1487))

## [0.4.2](https://github.com/alloy-rs/alloy/releases/tag/v0.4.2) - 2024-10-01

### Miscellaneous Tasks

- Release 0.4.2

### Styling

- Use alloc ([#1405](https://github.com/alloy-rs/alloy/issues/1405))

## [0.4.1](https://github.com/alloy-rs/alloy/releases/tag/v0.4.1) - 2024-10-01

### Features

- [consensus] Bincode compatibility for EIP-7702 ([#1404](https://github.com/alloy-rs/alloy/issues/1404))

### Miscellaneous Tasks

- Release 0.4.1
- [consensus] Less derives for bincode compatible types ([#1401](https://github.com/alloy-rs/alloy/issues/1401))

## [0.4.0](https://github.com/alloy-rs/alloy/releases/tag/v0.4.0) - 2024-09-30

### Bug Fixes

- Advance buffer during 2718 decoding ([#1367](https://github.com/alloy-rs/alloy/issues/1367))
- Correct `encode_2718_len` for legacy transactions ([#1360](https://github.com/alloy-rs/alloy/issues/1360))
- Enforce correct parity encoding for typed transactions ([#1305](https://github.com/alloy-rs/alloy/issues/1305))

### Features

- [consensus] Bincode compatibility for header and transaction types ([#1397](https://github.com/alloy-rs/alloy/issues/1397))
- Impl From<Eip2718Error> for alloy_rlp::Error ([#1359](https://github.com/alloy-rs/alloy/issues/1359))
- Add Header::num_hash_slow ([#1357](https://github.com/alloy-rs/alloy/issues/1357))
- [consensus] Generic Block Type ([#1319](https://github.com/alloy-rs/alloy/issues/1319))
- [consensus] Move requests struct definition from reth ([#1326](https://github.com/alloy-rs/alloy/issues/1326))

### Miscellaneous Tasks

- Release 0.4.0
- Rm outdated comments ([#1392](https://github.com/alloy-rs/alloy/issues/1392))

### Other

- Add supertrait alloy_consensus::Transaction to RPC TransactionResponse ([#1387](https://github.com/alloy-rs/alloy/issues/1387))
- Return static `Eip658Value` from `TxReceipt` trait method ([#1394](https://github.com/alloy-rs/alloy/issues/1394))
- Auto-impl `alloy_consensus::TxReceipt` for ref ([#1395](https://github.com/alloy-rs/alloy/issues/1395))
- Make `gas_limit` u64 for transactions ([#1382](https://github.com/alloy-rs/alloy/issues/1382))
- Make `Header` blob fees u64 ([#1377](https://github.com/alloy-rs/alloy/issues/1377))
- Make `Header` `base_fee_per_gas` u64 ([#1375](https://github.com/alloy-rs/alloy/issues/1375))
- Make `Header` gas limit u64 ([#1333](https://github.com/alloy-rs/alloy/issues/1333))
- Add `Receipts` struct ([#1247](https://github.com/alloy-rs/alloy/issues/1247))
- Add full feature to `derive_more` ([#1335](https://github.com/alloy-rs/alloy/issues/1335))
- Add `BlockHeader` getter trait ([#1302](https://github.com/alloy-rs/alloy/issues/1302))
- Implement custom default for `Account` representing a valid empty account ([#1313](https://github.com/alloy-rs/alloy/issues/1313))

## [0.3.6](https://github.com/alloy-rs/alloy/releases/tag/v0.3.6) - 2024-09-18

### Miscellaneous Tasks

- Release 0.3.6
- Release 0.3.5

## [0.3.4](https://github.com/alloy-rs/alloy/releases/tag/v0.3.4) - 2024-09-13

### Miscellaneous Tasks

- Release 0.3.4
- [consensus] Remove Header Method ([#1271](https://github.com/alloy-rs/alloy/issues/1271))
- [consensus] Alloc by Default ([#1272](https://github.com/alloy-rs/alloy/issues/1272))

### Other

- Implement `seal` helper for `Header` ([#1269](https://github.com/alloy-rs/alloy/issues/1269))

## [0.3.3](https://github.com/alloy-rs/alloy/releases/tag/v0.3.3) - 2024-09-10

### Miscellaneous Tasks

- Release 0.3.3
- Require destination for 7702 ([#1262](https://github.com/alloy-rs/alloy/issues/1262))

### Other

- Implement `AsRef` for `Header` ([#1260](https://github.com/alloy-rs/alloy/issues/1260))

## [0.3.2](https://github.com/alloy-rs/alloy/releases/tag/v0.3.2) - 2024-09-09

### Bug Fixes

- [consensus] Remove Unused Alloc Vecs ([#1250](https://github.com/alloy-rs/alloy/issues/1250))

### Miscellaneous Tasks

- Release 0.3.2

### Other

- Impl `exceeds_allowed_future_timestamp` for `Header` ([#1237](https://github.com/alloy-rs/alloy/issues/1237))
- Impl `is_zero_difficulty` for `Header` ([#1236](https://github.com/alloy-rs/alloy/issues/1236))
- Impl parent_num_hash for Header ([#1238](https://github.com/alloy-rs/alloy/issues/1238))
- Implement `Arbitrary` for `Header` ([#1235](https://github.com/alloy-rs/alloy/issues/1235))

## [0.3.1](https://github.com/alloy-rs/alloy/releases/tag/v0.3.1) - 2024-09-02

### Bug Fixes

- Value of TxEip1559.ty ([#1210](https://github.com/alloy-rs/alloy/issues/1210))

### Features

- Derive `arbitrary::Arbitrary` for `TxEip7702` ([#1216](https://github.com/alloy-rs/alloy/issues/1216))
- Implement `tx_type` for `TxEip7702` ([#1214](https://github.com/alloy-rs/alloy/issues/1214))

### Miscellaneous Tasks

- Release 0.3.1

### Other

- Rm useless methods for `TxEip7702` ([#1221](https://github.com/alloy-rs/alloy/issues/1221))

## [0.3.0](https://github.com/alloy-rs/alloy/releases/tag/v0.3.0) - 2024-08-28

### Dependencies

- Rm 2930 and 7702 - use alloy-rs/eips ([#1181](https://github.com/alloy-rs/alloy/issues/1181))

### Features

- Make signature methods generic over EncodableSignature ([#1138](https://github.com/alloy-rs/alloy/issues/1138))
- Add 7702 tx enum ([#1059](https://github.com/alloy-rs/alloy/issues/1059))
- Use EncodableSignature for tx encoding ([#1100](https://github.com/alloy-rs/alloy/issues/1100))
- [consensus] Add `From<ConsolidationRequest>` for `Request` ([#1083](https://github.com/alloy-rs/alloy/issues/1083))
- Expose encoded_len_with_signature() ([#1063](https://github.com/alloy-rs/alloy/issues/1063))
- Add 7702 tx type ([#1046](https://github.com/alloy-rs/alloy/issues/1046))
- Impl `arbitrary` for tx structs ([#1050](https://github.com/alloy-rs/alloy/issues/1050))

### Miscellaneous Tasks

- Release 0.3.0
- [consensus] Add missing getter trait methods for `alloy_consensus::Transaction` ([#1197](https://github.com/alloy-rs/alloy/issues/1197))
- Release 0.2.1
- Chore : fix typos ([#1087](https://github.com/alloy-rs/alloy/issues/1087))
- Release 0.2.0

### Other

- Add trait methods for constructing `alloy_rpc_types_eth::Transaction` to `alloy_consensus::Transaction` ([#1172](https://github.com/alloy-rs/alloy/issues/1172))
- Update TxType comment ([#1175](https://github.com/alloy-rs/alloy/issues/1175))
- Add payload length methods ([#1152](https://github.com/alloy-rs/alloy/issues/1152))
- `alloy-consensus` should use `alloy_primitives::Sealable` ([#1072](https://github.com/alloy-rs/alloy/issues/1072))

### Styling

- Remove proptest in all crates and Arbitrary derives ([#966](https://github.com/alloy-rs/alloy/issues/966))

## [0.1.4](https://github.com/alloy-rs/alloy/releases/tag/v0.1.4) - 2024-07-08

### Documentation

- Copy/paste error of eip-7251 link ([#961](https://github.com/alloy-rs/alloy/issues/961))

### Features

- Impl Transaction for TxEnvelope ([#1006](https://github.com/alloy-rs/alloy/issues/1006))
- Add eip-7702 helpers ([#950](https://github.com/alloy-rs/alloy/issues/950))

### Miscellaneous Tasks

- Release 0.1.4
- Release 0.1.3
- [eips] Make `sha2` optional, add `kzg-sidecar` feature ([#949](https://github.com/alloy-rs/alloy/issues/949))

### Other

- Remove signature.v parity before calculating tx hash ([#893](https://github.com/alloy-rs/alloy/issues/893))

## [0.1.2](https://github.com/alloy-rs/alloy/releases/tag/v0.1.2) - 2024-06-19

### Bug Fixes

- Make test compile ([#873](https://github.com/alloy-rs/alloy/issues/873))
- Support pre-658 status codes ([#848](https://github.com/alloy-rs/alloy/issues/848))
- Add request mod back ([#796](https://github.com/alloy-rs/alloy/issues/796))
- Make eip-7685 req untagged ([#743](https://github.com/alloy-rs/alloy/issues/743))
- Account for requests root in header mem size ([#706](https://github.com/alloy-rs/alloy/issues/706))
- Add check before allocation in `SimpleCoder::decode_one()` ([#689](https://github.com/alloy-rs/alloy/issues/689))
- [consensus] `TxEip4844Variant::into_signed` RLP ([#596](https://github.com/alloy-rs/alloy/issues/596))
- Add more generics to any and receipt with bloom ([#559](https://github.com/alloy-rs/alloy/issues/559))
- Change `Header::nonce` to `B64` ([#485](https://github.com/alloy-rs/alloy/issues/485))
- Infinite loop while decoding a list of transactions ([#432](https://github.com/alloy-rs/alloy/issues/432))
- Mandatory `to` on `TxEip4844` ([#355](https://github.com/alloy-rs/alloy/issues/355))
- Use enveloped encoding for typed transactions ([#239](https://github.com/alloy-rs/alloy/issues/239))
- Add encode_for_signing to Transaction, fix Ledger sign_transaction ([#161](https://github.com/alloy-rs/alloy/issues/161))
- [`consensus`] Ensure into_signed forces correct format for eip1559/2930 txs ([#150](https://github.com/alloy-rs/alloy/issues/150))
- [`eips`/`consensus`] Correctly decode txs on `TxEnvelope` ([#148](https://github.com/alloy-rs/alloy/issues/148))
- [consensus] Correct TxType flag in EIP-2718 encoding ([#138](https://github.com/alloy-rs/alloy/issues/138))
- [`consensus`] Populate chain id when decoding signed legacy txs ([#137](https://github.com/alloy-rs/alloy/issues/137))

### Dependencies

- [deps] Update all dependencies ([#258](https://github.com/alloy-rs/alloy/issues/258))
- Alloy-consensus crate ([#83](https://github.com/alloy-rs/alloy/issues/83))

### Documentation

- Add per-crate changelogs ([#914](https://github.com/alloy-rs/alloy/issues/914))
- Update descriptions and top level summary ([#128](https://github.com/alloy-rs/alloy/issues/128))

### Features

- Add eip-7251 consolidation request ([#919](https://github.com/alloy-rs/alloy/issues/919))
- Derive serde for header ([#902](https://github.com/alloy-rs/alloy/issues/902))
- Move `{,With}OtherFields` to serde crate ([#892](https://github.com/alloy-rs/alloy/issues/892))
- Add as_ is_ functions to envelope ([#872](https://github.com/alloy-rs/alloy/issues/872))
- Put wasm-bindgen-futures dep behind the `wasm-bindgen` feature flag ([#795](https://github.com/alloy-rs/alloy/issues/795))
- [serde] Deprecate individual num::* for a generic `quantity` module ([#855](https://github.com/alloy-rs/alloy/issues/855))
- Feat(consensus) Add test for account  ([#801](https://github.com/alloy-rs/alloy/issues/801))
- Feat(consensus) implement RLP for Account information ([#789](https://github.com/alloy-rs/alloy/issues/789))
- [`provider`] `eth_getAccount` support ([#760](https://github.com/alloy-rs/alloy/issues/760))
- Derive proptest arbitrary for `Request` ([#732](https://github.com/alloy-rs/alloy/issues/732))
- Serde for `Request` ([#731](https://github.com/alloy-rs/alloy/issues/731))
- Derive arbitrary for `Request` ([#729](https://github.com/alloy-rs/alloy/issues/729))
- Rlp enc/dec for requests ([#728](https://github.com/alloy-rs/alloy/issues/728))
- [consensus, eips] EIP-7002 system contract ([#727](https://github.com/alloy-rs/alloy/issues/727))
- Add eth mainnet EL requests envelope ([#707](https://github.com/alloy-rs/alloy/issues/707))
- Add eip-7685 requests root to header ([#668](https://github.com/alloy-rs/alloy/issues/668))
- Use alloy types for BlobTransactionSidecar ([#673](https://github.com/alloy-rs/alloy/issues/673))
- Passthrough methods on txenvelope ([#598](https://github.com/alloy-rs/alloy/issues/598))
- Add the txhash getter. ([#574](https://github.com/alloy-rs/alloy/issues/574))
- Refactor request builder workflow ([#431](https://github.com/alloy-rs/alloy/issues/431))
- Export inner encoding / decoding functions from `Tx*` types ([#529](https://github.com/alloy-rs/alloy/issues/529))
- `std` feature flag for `alloy-consensus` ([#461](https://github.com/alloy-rs/alloy/issues/461))
- Receipt qol functions ([#459](https://github.com/alloy-rs/alloy/issues/459))
- Add AnyReceiptEnvelope ([#446](https://github.com/alloy-rs/alloy/issues/446))
- Embed primitives Log in rpc Log and consensus Receipt in rpc Receipt ([#396](https://github.com/alloy-rs/alloy/issues/396))
- Serde for consensus tx types ([#361](https://github.com/alloy-rs/alloy/issues/361))
- Re-export EnvKzgSettings ([#375](https://github.com/alloy-rs/alloy/issues/375))
- Versioned hashes without kzg ([#360](https://github.com/alloy-rs/alloy/issues/360))
- `impl TryFrom<Transaction> for TxEnvelope` ([#343](https://github.com/alloy-rs/alloy/issues/343))
- 4844 SidecarBuilder ([#250](https://github.com/alloy-rs/alloy/issues/250))
- Derive `Hash` for `TypedTransaction` ([#284](https://github.com/alloy-rs/alloy/issues/284))
- Network abstraction and transaction builder ([#190](https://github.com/alloy-rs/alloy/issues/190))
- [`consensus`] Add extra EIP-4844 types needed ([#229](https://github.com/alloy-rs/alloy/issues/229))
- [`alloy-consensus`] `EIP4844` tx support ([#185](https://github.com/alloy-rs/alloy/issues/185))

### Miscellaneous Tasks

- Release 0.1.2
- Update changelogs for v0.1.1 ([#922](https://github.com/alloy-rs/alloy/issues/922))
- Add docs.rs metadata to all manifests ([#917](https://github.com/alloy-rs/alloy/issues/917))
- [clippy] Apply lint suggestions ([#903](https://github.com/alloy-rs/alloy/issues/903))
- Rm unused txtype mod ([#879](https://github.com/alloy-rs/alloy/issues/879))
- [other] Use type aliases where possible to improve clarity  ([#859](https://github.com/alloy-rs/alloy/issues/859))
- [docs] Crate completeness and fix typos ([#861](https://github.com/alloy-rs/alloy/issues/861))
- [docs] Add doc aliases ([#843](https://github.com/alloy-rs/alloy/issues/843))
- Fix remaining warnings, add TODO for proptest-derive ([#819](https://github.com/alloy-rs/alloy/issues/819))
- [consensus] Re-export EIP-4844 transactions ([#777](https://github.com/alloy-rs/alloy/issues/777))
- Remove rlp encoding for `Request` ([#751](https://github.com/alloy-rs/alloy/issues/751))
- Move blob validation to sidecar ([#677](https://github.com/alloy-rs/alloy/issues/677))
- Clippy, warnings ([#504](https://github.com/alloy-rs/alloy/issues/504))
- Improve hyper http error messages ([#469](https://github.com/alloy-rs/alloy/issues/469))
- Dedupe blob in consensus and rpc ([#401](https://github.com/alloy-rs/alloy/issues/401))
- Clean up kzg and features ([#386](https://github.com/alloy-rs/alloy/issues/386))

### Other

- [Fix] use Eip2718Error, add docs on different encodings ([#869](https://github.com/alloy-rs/alloy/issues/869))
- Add clippy at workspace level ([#766](https://github.com/alloy-rs/alloy/issues/766))
- Update clippy warnings ([#765](https://github.com/alloy-rs/alloy/issues/765))
- Arbitrary Sidecar implementation + build. Closes [#680](https://github.com/alloy-rs/alloy/issues/680). ([#708](https://github.com/alloy-rs/alloy/issues/708))
- Use into instead of from ([#749](https://github.com/alloy-rs/alloy/issues/749))
- Correctly sign non legacy transaction without EIP155 ([#647](https://github.com/alloy-rs/alloy/issues/647))
- Some refactoring ([#739](https://github.com/alloy-rs/alloy/issues/739))
- Replace into_receipt by into ([#735](https://github.com/alloy-rs/alloy/issues/735))
- Replace into_tx by into ([#737](https://github.com/alloy-rs/alloy/issues/737))
- Use Self when possible ([#711](https://github.com/alloy-rs/alloy/issues/711))
- Use `From<Address>` for `TxKind` ([#651](https://github.com/alloy-rs/alloy/issues/651))
- Extension ([#474](https://github.com/alloy-rs/alloy/issues/474))
- TypeTransaction conversion trait impls ([#472](https://github.com/alloy-rs/alloy/issues/472))
- Mark envelopes non-exhaustive ([#456](https://github.com/alloy-rs/alloy/issues/456))
- Numeric type audit: network, consensus, provider, rpc-types ([#454](https://github.com/alloy-rs/alloy/issues/454))
- Check no_std in CI ([#367](https://github.com/alloy-rs/alloy/issues/367))

### Refactor

- Refactor around TxEip4844Variant ([#738](https://github.com/alloy-rs/alloy/issues/738))
- Clean up legacy serde helpers ([#624](https://github.com/alloy-rs/alloy/issues/624))

### Styling

- Make additional TxReceipt impls generic over T ([#617](https://github.com/alloy-rs/alloy/issues/617))
- [Feature] Receipt trait in alloy-consensus ([#477](https://github.com/alloy-rs/alloy/issues/477))
- Sort derives ([#499](https://github.com/alloy-rs/alloy/issues/499))
- Implement `arbitrary` for `TransactionReceipt` ([#449](https://github.com/alloy-rs/alloy/issues/449))

[`alloy`]: https://crates.io/crates/alloy
[alloy]: https://crates.io/crates/alloy
[`alloy-core`]: https://crates.io/crates/alloy-core
[alloy-core]: https://crates.io/crates/alloy-core
[`alloy-consensus`]: https://crates.io/crates/alloy-consensus
[alloy-consensus]: https://crates.io/crates/alloy-consensus
[`alloy-contract`]: https://crates.io/crates/alloy-contract
[alloy-contract]: https://crates.io/crates/alloy-contract
[`alloy-eips`]: https://crates.io/crates/alloy-eips
[alloy-eips]: https://crates.io/crates/alloy-eips
[`alloy-genesis`]: https://crates.io/crates/alloy-genesis
[alloy-genesis]: https://crates.io/crates/alloy-genesis
[`alloy-json-rpc`]: https://crates.io/crates/alloy-json-rpc
[alloy-json-rpc]: https://crates.io/crates/alloy-json-rpc
[`alloy-network`]: https://crates.io/crates/alloy-network
[alloy-network]: https://crates.io/crates/alloy-network
[`alloy-node-bindings`]: https://crates.io/crates/alloy-node-bindings
[alloy-node-bindings]: https://crates.io/crates/alloy-node-bindings
[`alloy-provider`]: https://crates.io/crates/alloy-provider
[alloy-provider]: https://crates.io/crates/alloy-provider
[`alloy-pubsub`]: https://crates.io/crates/alloy-pubsub
[alloy-pubsub]: https://crates.io/crates/alloy-pubsub
[`alloy-rpc-client`]: https://crates.io/crates/alloy-rpc-client
[alloy-rpc-client]: https://crates.io/crates/alloy-rpc-client
[`alloy-rpc-types`]: https://crates.io/crates/alloy-rpc-types
[alloy-rpc-types]: https://crates.io/crates/alloy-rpc-types
[`alloy-rpc-types-anvil`]: https://crates.io/crates/alloy-rpc-types-anvil
[alloy-rpc-types-anvil]: https://crates.io/crates/alloy-rpc-types-anvil
[`alloy-rpc-types-beacon`]: https://crates.io/crates/alloy-rpc-types-beacon
[alloy-rpc-types-beacon]: https://crates.io/crates/alloy-rpc-types-beacon
[`alloy-rpc-types-engine`]: https://crates.io/crates/alloy-rpc-types-engine
[alloy-rpc-types-engine]: https://crates.io/crates/alloy-rpc-types-engine
[`alloy-rpc-types-eth`]: https://crates.io/crates/alloy-rpc-types-eth
[alloy-rpc-types-eth]: https://crates.io/crates/alloy-rpc-types-eth
[`alloy-rpc-types-trace`]: https://crates.io/crates/alloy-rpc-types-trace
[alloy-rpc-types-trace]: https://crates.io/crates/alloy-rpc-types-trace
[`alloy-serde`]: https://crates.io/crates/alloy-serde
[alloy-serde]: https://crates.io/crates/alloy-serde
[`alloy-signer`]: https://crates.io/crates/alloy-signer
[alloy-signer]: https://crates.io/crates/alloy-signer
[`alloy-signer-aws`]: https://crates.io/crates/alloy-signer-aws
[alloy-signer-aws]: https://crates.io/crates/alloy-signer-aws
[`alloy-signer-gcp`]: https://crates.io/crates/alloy-signer-gcp
[alloy-signer-gcp]: https://crates.io/crates/alloy-signer-gcp
[`alloy-signer-ledger`]: https://crates.io/crates/alloy-signer-ledger
[alloy-signer-ledger]: https://crates.io/crates/alloy-signer-ledger
[`alloy-signer-local`]: https://crates.io/crates/alloy-signer-local
[alloy-signer-local]: https://crates.io/crates/alloy-signer-local
[`alloy-signer-trezor`]: https://crates.io/crates/alloy-signer-trezor
[alloy-signer-trezor]: https://crates.io/crates/alloy-signer-trezor
[`alloy-signer-wallet`]: https://crates.io/crates/alloy-signer-wallet
[alloy-signer-wallet]: https://crates.io/crates/alloy-signer-wallet
[`alloy-transport`]: https://crates.io/crates/alloy-transport
[alloy-transport]: https://crates.io/crates/alloy-transport
[`alloy-transport-http`]: https://crates.io/crates/alloy-transport-http
[alloy-transport-http]: https://crates.io/crates/alloy-transport-http
[`alloy-transport-ipc`]: https://crates.io/crates/alloy-transport-ipc
[alloy-transport-ipc]: https://crates.io/crates/alloy-transport-ipc
[`alloy-transport-ws`]: https://crates.io/crates/alloy-transport-ws
[alloy-transport-ws]: https://crates.io/crates/alloy-transport-ws

<!-- generated by git-cliff -->
