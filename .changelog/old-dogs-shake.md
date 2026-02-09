---
reth-transaction-pool: patch
---

Added `IntoIter: Send` bounds to `validate_transactions` and `validate_transactions_with_origin` method signatures to ensure proper Send trait bounds for async validation operations.
