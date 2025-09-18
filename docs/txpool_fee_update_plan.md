# TxPool Fee Update Refactor Plan

## Context and Goals
- `apply_fee_updates` duplicates the promotion logic already embedded inside `update_basefee` and `update_blob_fee`.
- We want `on_canonical_state_change` to reuse the same promotion/demotion code paths as `set_block_info` while still preventing double processing of fee-induced transitions.
- The end state should expose a single source of truth for fee-driven state transitions and allow callers that care about promoted transactions to supply an `UpdateOutcome` sink.

## Proposed Refactor
1. **Factor promotion helpers** ✅
   - Extracted base-fee decrease branch into `handle_basefee_decrease` and blob-fee branch into `handle_blob_fee_decrease`, ensuring state bits and routing invariants stay intact.

2. **Reuse helpers inside existing fee update entry points** ✅
   - `update_basefee` and `update_blob_fee` now delegate to the shared helpers for their respective decrease branches.

3. **Teach canonical-state path to reuse helpers** ✅
   - `apply_fee_updates` invokes the helpers and feeds promotions into the supplied `UpdateOutcome`, keeping the existing guard conditions to prevent double processing.

4. **Update supporting utilities** ✅
   - No changes required for `update_pending_fees_only`; added unit coverage for both base-fee and blob-fee drop scenarios to ensure promotions are reported once.

## Validation Plan
- ✅ Ran existing txpool unit/integration tests (`cargo test -p reth-transaction-pool`).
- ✅ Added targeted unit tests asserting canonical-state promotion reporting for base-fee and blob-fee drops.

## Rollout Notes
- No public API changes expected; refactor is internal to the txpool.
- Monitor downstream call sites for any compilation errors due to the new helper signatures during implementation; adjust visibility (`pub(crate)`) accordingly.
