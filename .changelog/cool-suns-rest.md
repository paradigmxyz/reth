---
reth-transaction-pool: minor
---

Added support for optional custom stateless and stateful validation hooks in `EthTransactionValidator` via `set_additional_stateless_validation` and `set_additional_stateful_validation` methods. Also implemented a manual `Debug` impl to handle the non-`Debug` function pointer fields.
