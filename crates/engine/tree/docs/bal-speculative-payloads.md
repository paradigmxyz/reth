# BAL Speculative Payload Building Foundation

This note documents the reth-side primitives added for downstream nodes that want to build a child
payload over a parent block access list (BAL) while that parent is still validating. The normal reth
path remains canonical-parent-hash based unless these APIs are used explicitly.

## Implemented In Reth

### BAL final-write overlay

`reth_chain_state::BalStateOverlay` constructs a final-write state overlay from decoded EIP-7928
BAL account changes. The overlay records only writes and ignores BAL reads, so prefetch-only reads
cannot override final state. It covers account creation and deletion, balance, nonce, code hash,
code bytes, empty code, storage writes, zero values, and repeated writes.

The overlay is composed with an existing state provider through
`BalStateOverlay::provider(fallback)`. Overlay reads are served first, untouched state falls back to
the supplied provider, and deleted accounts return no account with zero storage for overlaid slots.
This abstraction is independent of Tempo and can be used by payload builders, prewarmers, or tests.

### Speculative payload-building API

`reth_payload_builder::BuildNewPayloadWithState` wraps the existing `BuildNewPayload` with a
`PayloadStateAnchor`. The default anchor is `PayloadStateAnchor::Canonical`, which preserves the
existing parent-hash lookup behavior.

Downstream code can opt in with `PayloadStateAnchor::Speculative(SpeculativePayloadState)`.
`SpeculativePayloadState` can carry:

- `PayloadValidityToken`, a shared validity dependency for the parent being built on.
- `SpeculativeStateProvider`, a factory that opens the state provider used by each build attempt.

`PayloadBuilderHandle::send_new_payload_with_state_anchor` sends the anchored request to the
payload-builder service. `PayloadJobGenerator::new_payload_job_with_state` and the basic payload job
now carry the anchor into build attempts, missing-payload fallback, and empty-payload construction.
The Ethereum payload builder opens the supplied speculative provider when present; otherwise it uses
the canonical `state_by_block_hash` path.

If the validity token is marked invalid before or during an Ethereum build attempt, that attempt
returns `BuildOutcome::Cancelled`. The token does not make speculative payloads publishable; it only
models the dependency and gives builders a common cancellation check.

### Execution-cache lease generations

`PayloadExecutionCache` now treats the published cache as a generation. Matching parent-hash
checkouts can overlap, so current-payload validation and speculative builders may hold lookup leases
at the same time. A parent-hash mismatch no longer clears or retags the published generation; the
caller creates an isolated cache instead.

`SavedCache` exposes its generation, a `CacheLeaseTracker`, and
`fork_empty_with_hash(hash)` for preparing a child generation without mutating the current one.
`PayloadExecutionCache::publish_cache_update` publishes a new generation only after an optional
validity receiver succeeds and any supplied previous-generation leases are released. If validity
fails, the new generation is discarded and the published generation is left intact.

BAL and transaction prewarm cache saving now inserts final block state into a fresh child generation
and publishes it through this lifecycle instead of mutating or clearing the active generation in
place.

## Downstream Usage

A downstream builder that has validated enough of parent `B`'s BAL can:

1. Decode the BAL and build `BalStateOverlay::from_bal(&bal)`.
2. Open the canonical parent provider for `B`'s parent and compose it with
   `overlay.provider(parent_provider)`.
3. Wrap that provider in `SpeculativeStateProvider::new("label", move || Ok(Box::new(...)))`.
4. Create `PayloadValidityToken::pending(B_hash)` and attach it through
   `SpeculativePayloadState::new(token.clone()).with_state_provider(provider)`.
5. Call `PayloadBuilderHandle::send_new_payload_with_state_anchor` for `B+1`.
6. Mark the token valid only after `B` is accepted; mark it invalid if validation fails so in-flight
   speculative work can cancel and discard local results.

Cache users that produce a post-parent cache generation should build the new `SavedCache` separately
and use `publish_cache_update` with the old generation's `CacheLeaseTracker` and any parent-validity
dependency.

## Follow-Up Integration Work

Tempo or another downstream node still needs to implement the scheduling policy that decides when to
start the speculative `B+1` job, how to assemble the BAL overlay from its parent validation pipeline,
and how to prevent a speculative payload from being served as canonical before the parent validity
dependency resolves.

Sparse-trie speculative publication follows the same lifecycle requirement but is not enabled by
default here. Downstream integration should keep speculative trie results separate until parent
validity succeeds and active old-generation users are done.

No consensus-visible forkchoice behavior changed in reth. Forkchoice still creates normal payload
jobs through `BuildNewPayload`, and speculative building is only reachable through the explicit
state-anchor APIs.
