---
title: Make it easier to optimize batch tx validations
labels:
    - A-sdk
    - A-tx-pool
    - C-enhancement
    - C-perf
assignees:
    - yongkangc
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.998643Z
info:
    author: klkvr
    created_at: 2025-09-17T15:49:07Z
    updated_at: 2025-09-26T04:48:05Z
---

### Describe the feature

In `EthTransactionValidator` we do:
https://github.com/paradigmxyz/reth/blob/4b4b122e75182985088a523beb192c5f47b9975c/crates/transaction-pool/src/validate/eth.rs#L774-L787
https://github.com/paradigmxyz/reth/blob/4b4b122e75182985088a523beb192c5f47b9975c/crates/transaction-pool/src/validate/eth.rs#L700-L710

Which allows to avoid overhead of instantiating state provider multiple times while also reusing code for transaction validation in all cases.

Currently this optimization is not applied for e.g `OpTransactionValidator` which still calls `validate_one` for each of the transactions
https://github.com/paradigmxyz/reth/blob/4b4b122e75182985088a523beb192c5f47b9975c/crates/optimism/txpool/src/validator.rs#L309-L317

What we could do is bake this optimization into `TransactionValidator` like this:
```rust
trait TransactionValidator {
    /// Performs validations not requiring state access.
    fn validate_transaction_stateless(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> Result<Self::Transaction, TransactionValidationOutcome<Self::Transaction>>;

    /// Validates transaction against latest state
    fn validate_transaction_stateful(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
        state: impl StateProvider
    ) -> TransactionValidationOutcome<Self::Transaction>;
}
```

And then we could have `TransactionValidationTaskExecutor` take care of instantiating provider only when one or more transactions succeeded `validate_transaction_stateless`

### Additional context

_No response_
