---
reth-transaction-pool: patch
---

Fixed a bug where transactions from the same sender were added to the pending subpool out of nonce order. Ensured `process_updates` runs before `add_new_transaction` so that lower-nonce promotions are enqueued before the newly inserted higher-nonce transaction, preserving correct ordering for live `BestTransactions` iterators.
