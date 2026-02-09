---
reth-transaction-pool: minor
---

Added `IntoIter: Send` bounds to `validate_transactions` and `validate_transactions_with_origin` in the `TransactionValidator` trait, avoiding unnecessary `Vec` collects. Simplified default `validate_transactions_with_origin` to delegate to `validate_transactions`.
