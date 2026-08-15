use alloy_eips::Encodable2718;
use alloy_primitives::Bytes;
use reth_transaction_pool::{
    test_utils::{MockTransaction, MockTransactionFactory, TestPoolBuilder},
    PoolTransaction, TransactionOrigin, TransactionPool,
};

#[tokio::test(flavor = "multi_thread")]
async fn build_inclusion_list_enforces_rlp_list_size_limit() {
    let pool = TestPoolBuilder::default();
    let mut factory = MockTransactionFactory::default();

    let tx1 = factory.validated(MockTransaction::legacy().with_gas_price(1_000_000_000u128));
    let tx2 = factory.validated(MockTransaction::legacy().with_gas_price(1_000_000_000u128));
    let _ = pool.add_transaction(TransactionOrigin::External, tx1.transaction.clone()).await;
    let _ = pool.add_transaction(TransactionOrigin::External, tx2.transaction.clone()).await;

    let all = pool.build_inclusion_list(usize::MAX);
    assert_eq!(all.len(), 2);

    let max_size = alloy_rlp::list_length::<Bytes, [u8]>(&all).saturating_sub(1);
    let limited = pool.build_inclusion_list(max_size);
    assert!(limited.len() < all.len());
    assert!(alloy_rlp::list_length::<Bytes, [u8]>(&limited) <= max_size);
}

#[tokio::test(flavor = "multi_thread")]
async fn build_inclusion_list_excludes_blob_transactions() {
    let pool = TestPoolBuilder::default();
    let mut factory = MockTransactionFactory::default();

    let blob = factory.validated(
        MockTransaction::eip4844().with_max_fee(1_000_000_000u128).with_blob_fee(1_000_000_000u128),
    );
    let non_blob = factory.validated(MockTransaction::eip1559().with_max_fee(1_000_000_000u128));
    let expected = non_blob.transaction.clone().into_consensus().into_inner().encoded_2718();

    let _ = pool.add_transaction(TransactionOrigin::External, blob.transaction).await;
    let _ = pool.add_transaction(TransactionOrigin::External, non_blob.transaction).await;

    assert_eq!(pool.build_inclusion_list(usize::MAX), vec![Bytes::from(expected)]);
}
