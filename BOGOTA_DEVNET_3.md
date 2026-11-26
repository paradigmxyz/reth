# Bogotá Devnet 3

This branch builds on Devnet 2 at 954168ed43 and the dependency revisions recorded there.
This is the PROOF candidate implementation, not a finalized Devnet 3 release.
The protocol target is EIP-8141 draft 9c7074e01b8698e057aa46363761a902f4e3edd8.
The release tracker is https://github.com/ethereum/execution-specs/issues/4069.
PQTS 19 calls for leanSPHINCS without builder or mempool aggregation.

The tracker rechecked on 2026-11-26 now explicitly leaves the choice between
Claims List, PROOF frames, and EIP-8504 open for ACDT. Its later statement that
PROOF is the only spec change is stale and contradicts that decision status.
The pinned PROOF draft head remains unchanged. Final devnet interoperability
cannot be claimed until the candidate is selected and fixtures are released.

## Published sources

All external workspace dependencies are pinned to exact published commits.
The local checkouts are on bogota-devnet-3; a fresh checkout of this branch
resolves these sources without sibling directories or path overrides.

| Dependency | Commit |
| --- | --- |
| eips | 621371a6937ccd301d16b1e877887786b0f5b2ae |
| alloy | 9dd77cd5192394e8712201a902130c581a5c12ee |
| evm2 | 1ad2a83a290159cb2b21d2d1a6c0ce119f02b8db |
| reth-core | 009e52888f187aa3cd27737d7257ae3e23069865 |

The leanSPHINCS verifier is also pinned by commit. Development used the isolated
D:/reth-oss-bogota-devnet-3 worktree; D:/reth-oss belongs to another task.

## Envelope and execution

The keyed-nonce payload is [chain_id, nonce_keys, nonce_seq, sender, frames, fees,
blob_versioned_hashes]. There is no signatures list. Proof mode is 4; ASSERT mode
remains 3. Proof frames lead the transaction, execute in a pure context, and charge
their entire execution allocation. Their witnesses are priced by length. Ordinary
frame execution, state-gas ownership, assertion rollback, and nonce domains are retained.

Contract proof verifiers use a guarded interpreter path even when a compiled runner
is configured. Native verifier precompiles run directly. Proof frames do not warm
accounts for later frames. RPC-only simulation can waive an omitted witness for a
known native verifier through an explicit feature that is off for consensus and pool
validation. A supplied witness is always verified. Public-prefix validation checks
native claims before sender/nonce reads and reuses successful checks only within
the same immutable prepared payload.

## Provisional assignments and spec questions

The draft leaves verifier addresses TBD. This local devnet assigns secp256k1 to
0x12, P-256 to 0x13, and leanSPHINCS to 0x14, as explicitly approved for this task.
These are provisional devnet assignments, not agreed mainnet addresses.

EIP-8151 makes legacy ecRecover stateful, while the PROOF specification assumes
precompiles are pure. Until this interaction is specified, legacy ecRecover is
rejected in the pure context. The new secp256k1 verifier performs raw cryptographic
verification and does not inspect account code. This is an explicit fail-closed
integration choice requiring coordination before interoperable devnet deployment.

The draft rationale's contract-proof overhead example uses a 2,600-gas cold access
(3,075 with the per-frame charge). The Devnet 2 Amsterdam schedule includes EIP-8038's
3,000-gas cold access, making that overhead 3,475. Execution uses the active schedule,
as the normative rule requires; the example is stale for this base.

The EIP-8250 example on master still shows the retired signatures list and old fee
fields. Its nonce-field replacement is composed with the pinned EIP-8141 envelope;
its stale example is not used as the complete transaction schema.

The Devnet 2 base does not recognize canonical paymaster/helper runtime code. That
public-pool exception remains unsupported: generic trace restrictions and the
noncanonical payer cap continue to apply. The draft provides reference sources but
no agreed helper deployment address, compiler settings, or canonical runtime bytes;
recognizing arbitrary matching-looking contracts would bypass safety restrictions.
Canonical paymaster interoperability needs those deployment artifacts fixed first.

The pinned leanSPHINCS crate's conformance vectors reference EthLib 03547dd8,
whereas the draft names f0e41b61. Parameter, hashing, encoding, and verifier-helper
source comparisons found no incompatible verification change (the latter revision
refactors Merkle positions and signing tree construction). The native ABI is checked
against a valid EthLib-derived vector and mutations. This is not a full differential
execution against f0e41b61 and is not an independent cryptographic audit.

## Storage

Use a fresh Devnet 3 data directory. The transaction encoding and hashes differ from
Devnet 2. Compact proof transactions carry version 1; old signature-list records
are rejected explicitly, rather than interpreted as the new schema. No existing
node data directory is opened or migrated by this change. Compiled-artifact ABI
version 8 invalidates Devnet 2 JIT/AOT artifacts after the opcode and host-layout changes.

## Validation

Completed checks:

| Area | Result |
| --- | --- |
| Native EVM library and affected frame regressions | 981 passed |
| EIP-8141 type, encoding, serde, and legacy compatibility tests | 38 passed |
| Alloy consensus | Full 144-test library run passed; final 28 affected tests passed, including a new independent RLP vector |
| Alloy RPC/network libraries and integration | Passed; RPC library 133, network integration 17, provider submission 2 |
| Compact transaction codec | 3 passed, including rejection of Devnet 2 records |
| Public transaction-pool frame checks | 63 passed |
| Reth RPC frame checks | 18 passed |
| RPC type, simulation, and proof-padding helpers | 12 passed |
| Node end-to-end frame checks | 8 passed: pool admission, mining, block import, and keyed nonce roundtrips |
| JIT/AOT, artifact reload, and compilation reuse | 3 passed, including compiled PROOFPARAM after an interpreted proof |
| Frame tracing with JavaScript tracers | 33 passed |
| no_std and native Clippy with warnings denied | Passed |
| Dependency-feature and whitespace checks | Passed |

Windows cannot build Reth's pool unit-test storage dependencies because existing
provider APIs are Unix-gated; the Reth tests above ran in Ubuntu. One deep-call
parity test overflowed the default Windows test-thread stack; the complete tracing
group passed with RUST_MIN_STACK=16777216. Native EVM tests passed with the default
stack. The make lint-toml wrapper failed under the Windows shell; its dprint fmt
recipe ran successfully, and formatting-only changes outside this task were removed.

The official Devnet 3 fixture tag and devnet specification directory were unavailable
when checked. This branch therefore targets the pinned PROOF draft, with the explicit
integration gaps above, rather than claiming fixture-certified interoperability.
The bogota-devnet-3 branches are published without opening a pull request.
