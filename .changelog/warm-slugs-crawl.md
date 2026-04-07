---
reth-transaction-pool: minor
---

Added `TransactionValidationTaskExecutor::spawn` as a dedicated constructor that encapsulates spawning validation tasks on a runtime, and refactored `EthTransactionValidatorBuilder::build_with_tasks` to use it.
