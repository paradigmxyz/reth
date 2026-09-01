//! EIP-7805 (FOCIL) inclusion list building.

use reth_transaction_pool::{
    test_utils::{MockTransactionFactory, TestPoolBuilder},
    TransactionOrigin, TransactionPool, MAX_INCLUSION_LIST_TXS_PER_SENDER,
};

#[tokio::test(flavor = "multi_thread")]
async fn inclusion_list_is_empty_without_transactions() {
    let pool = TestPoolBuilder::default();
    assert!(pool.build_inclusion_list(8192).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn inclusion_list_caps_transactions_per_sender() {
    const OTHER_SENDERS: usize = 2;
    const BUSY_SENDER_TXS: usize = 6;

    let pool = TestPoolBuilder::default();
    let mut factory = MockTransactionFactory::default();

    // One sender with a long run of consecutive nonces. Only its next nonce is appendable, so
    // the list should take at most `MAX_INCLUSION_LIST_TXS_PER_SENDER` of them.
    let busy = factory.create_eip1559().transaction;
    pool.add_transaction(TransactionOrigin::External, busy.clone()).await.unwrap();
    let mut prev = busy;
    for _ in 1..BUSY_SENDER_TXS {
        let next = prev.rng_hash().inc_nonce();
        pool.add_transaction(TransactionOrigin::External, next.clone()).await.unwrap();
        prev = next;
    }
    for _ in 0..OTHER_SENDERS {
        let other = factory.create_eip1559().transaction;
        pool.add_transaction(TransactionOrigin::External, other).await.unwrap();
    }

    let il = pool.build_inclusion_list(8192);

    // Without the cap every pooled transaction would fit the byte budget.
    assert_eq!(
        il.len(),
        MAX_INCLUSION_LIST_TXS_PER_SENDER + OTHER_SENDERS,
        "expected the busy sender to be capped, got {} of {} pooled transactions",
        il.len(),
        BUSY_SENDER_TXS + OTHER_SENDERS,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn inclusion_list_respects_the_size_limit() {
    let pool = TestPoolBuilder::default();
    let mut factory = MockTransactionFactory::default();
    for _ in 0..8 {
        let tx = factory.create_eip1559().transaction;
        pool.add_transaction(TransactionOrigin::External, tx).await.unwrap();
    }

    // A budget too small for even one transaction yields an empty list.
    assert!(pool.build_inclusion_list(1).is_empty());

    let il = pool.build_inclusion_list(8192);
    assert!(!il.is_empty());
    assert!(alloy_rlp::list_length::<alloy_primitives::Bytes, [u8]>(&il) <= 8192);
}
