---
title: Ideas to improve `TransactionValidator`
labels:
    - A-tx-pool
    - C-perf
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.99033Z
info:
    author: hai-rise
    created_at: 2025-06-20T08:12:43Z
    updated_at: 2025-07-24T12:09:13Z
---

### Describe the feature

1. Remove `TransactionValidator::validate_transactions`

This doesn't seem to be used in Reth internals with `Pool::validate_all` using `validate_transactions_with_origin` instead. 

Even though it provides a default implementation, a `CustomTransactionValidator` wanting to reuse a provider for all transactions would still need to implement it for completeness. It's extra annoying given how similar it is to `validate_transactions_with_origin`. 

I'd say it's cleaner to remove `validate_transactions`, and only ask a `CustomTransactionValidator` (with a custom provider) to implement its custom `validate_transactions_with_origin`. And any custom validator wanting extra interfaces like this should implement directly in its structure, not via the trait.

---

2. `EthTransactionValidator`: Persist a state provider and only update once per block

The hottest validation path is per new mempool transaction: `Pool::add_transaction` -> `EthTransactionValidator::validate_transaction` -> `EthTransactionValidatorInner::validate_one_with_provider` without an existing provider.

This means calling `self.client.latest()` per new mempool transaction, which wouldn't scale well for chains with high traffic. Instead, a concrete `TransactionValidator` like `EthTransactionValidator` can persist a single state provider for reuse, and only update it per new canonical block (like via `TransactionValidator::on_new_head_block`).

---

These can lead to huge refactorings & public API changes, so opening an issue first for confirmation 🙏.

### Additional context

ref https://github.com/paradigmxyz/reth/issues/12811
