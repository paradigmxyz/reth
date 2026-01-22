---
title: BestTransactions may iterate transactions in nonce disorder
labels:
    - A-tx-pool
    - C-bug
assignees:
    - Troublor
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.993988Z
info:
    author: Troublor
    created_at: 2025-08-01T03:43:43Z
    updated_at: 2025-08-11T13:52:37Z
---

### Describe the bug

When a newly added transaction is added to the pending subpool, some other transactions may also need to be moved to the pending subpool. 
https://github.com/paradigmxyz/reth/blob/a5f2d58650664abea51f36a12dd3ae1c156d7579/crates/transaction-pool/src/pool/txpool.rs#L676-L702
The execution order is:
1. The new transaction is first added to the pending subpool.
2. Other transactions are moved to pending. 
The `PendingPool` will send the newly added transaction in insertion order to existing `BestTransactions` instances. If the transactions are not added in nonce order, the `BestTransactions` will receive and iterate transactions in a nonce disorder. 
The scenario that transactions are not added in nonce order is: when "other `updates` transactions` contain a transaction whose sender is the same of the newly added transaction but with la ower nonce. I will give an example below in the reproduction steps. 

The problematic symptom is: the transaction list output by `BestTransactions` does not adhere to the partial order by transaction nonce. 


### Steps to reproduce

1. Add this test in https://github.com/paradigmxyz/reth/blob/a5f2d58650664abea51f36a12dd3ae1c156d7579/crates/transaction-pool/src/pool/txpool.rs 
```rust

    #[test]
    fn test_insertion_disorder() {
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let sender = address!("1234567890123456789012345678901234567890");
        let tx0 = f.validated_arc(
            MockTransaction::legacy().with_sender(sender).with_nonce(0).with_gas_price(10),
        );
        let tx1 = f.validated_arc(
            MockTransaction::eip1559()
                .with_sender(sender)
                .with_nonce(1)
                .with_gas_limit(1000)
                .with_gas_price(10),
        );
        let tx2 = f.validated_arc(
            MockTransaction::legacy().with_sender(sender).with_nonce(2).with_gas_price(10),
        );
        let tx3 = f.validated_arc(
            MockTransaction::legacy().with_sender(sender).with_nonce(3).with_gas_price(10),
        );

        let mut best = pool.best_transactions();

        // tx0 should be put in the pending subpool
        pool.add_transaction((*tx0).clone(), U256::from(1000), 0, None).unwrap();
        let t0 = best.next().expect("tx0 should be put in the pending subpool");
        assert_eq!(t0.id(), tx0.id());
        // tx1 should be put in the queued subpool due to insufficient sender balance
        pool.add_transaction((*tx1).clone(), U256::from(1000), 0, None).unwrap();
        assert!(best.next().is_none());
        // tx2 should be put in the pending subpool, and tx1 should be promoted to pending
        pool.add_transaction((*tx2).clone(), U256::MAX, 0, None).unwrap();
        let t1 = best.next().expect("tx2 should be put in the pending subpool");
        let t2 = best.next().expect("tx2 should be put in the pending subpool");
        assert_eq!(t1.id(), tx1.id());
        assert_eq!(t2.id(), tx2.id());
        // tx3 should be put in the pending subpool,
        pool.add_transaction((*tx3).clone(), U256::MAX, 0, None).unwrap();
        let t3 = best.next().expect("tx3 should be put in the pending subpool");
        assert_eq!(t3.id(), tx3.id());
    }
```
The test does the following things:
1. It first adds a transaction `tx0` (nonce=0), directly put in the pending subpool. 
2. Then transaction `tx1` (nonce=1) is added, whose cost is higher than the sender's balance. It is put in the queued subpool. 
3. Transaction `tx2` (nonce=2) is added. In the meantime, the sender's balance is updated to be sufficiently large. `tx2` should be put in the pending subpool and `tx1` should be moved to the queued subpool. 
4. As the logic in `TxPool::add_transaction`, `tx2` will be first added to the subpool and then `tx1`. The existing `BestTransaction` instances will first receive `tx2` and then `tx1`. 
5. As a result, the `BestTransaction` iterator will first output `tx2` and then `tx1`, causing a nonce gap in the output transaction list. 

### Node logs

```text
N/A
```

### Platform(s)

Linux (x86)

### Container Type

Not running in a container

### What version/commit are you on?

git commit `54a4a23f64a2522903f9aafb02d2b65f5a45ad7d`

### What database version are you on?

N/A

### Which chain / network are you on?

N/A

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
