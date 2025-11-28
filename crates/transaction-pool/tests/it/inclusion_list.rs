/// Tests for inclusion list functionality in the transaction pool.
use alloy_eips::Encodable2718;
use reth_transaction_pool::{
    test_utils::{MockTransaction, MockTransactionFactory, TestPoolBuilder},
    PoolTransaction, TransactionOrigin, TransactionPool,
};

#[tokio::test(flavor = "multi_thread")]
async fn test_build_inclusion_list_with_transactions() {
    let pool = TestPoolBuilder::default();
    let mut factory = MockTransactionFactory::default();

    // Create some test transactions
    let tx1 = factory.validated(MockTransaction::eip1559().with_max_fee(1_000_000_000u128));
    let tx2 = factory.validated(MockTransaction::legacy().with_gas_price(1_000_000_000u128));

    // Add transactions to pool
    let _ = pool.add_transaction(TransactionOrigin::External, tx1.transaction.clone()).await;
    let _ = pool.add_transaction(TransactionOrigin::External, tx2.transaction.clone()).await;

    // Build inclusion list
    let il = pool.build_inclusion_list(1_000_000_000, 8192);

    assert!(
        il.len() == 2,
        "Inclusion list should not exceed number of inserted non-blob transactions"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_build_inclusion_list_respects_size_limit() {
    let pool = TestPoolBuilder::default();
    let mut factory = MockTransactionFactory::default();

    let tx1 = factory.validated(MockTransaction::legacy().with_gas_price(1_000_000_000u128));
    let tx2 = factory.validated(MockTransaction::legacy().with_gas_price(1_000_000_000u128));

    let consensus_tx = tx1.transaction.clone().into_consensus().into_inner();
    let tx_sz = consensus_tx.encoded_2718().len();

    let _ = pool.add_transaction(TransactionOrigin::External, tx1.transaction.clone()).await;
    let _ = pool.add_transaction(TransactionOrigin::External, tx2.transaction.clone()).await;

    // Too small of a size limit
    let il_small = pool.build_inclusion_list(1_000_000_000, tx_sz.saturating_sub(1));
    assert!(il_small.is_empty(), "Inclusion list must respect a tiny size limit");

    // Just enough of a size limit
    let il_large = pool.build_inclusion_list(1_000_000_000, tx_sz);
    assert!(
        il_large.len() == 1,
        "There should only be room for one Tx. IL Size: {:?}",
        il_large.len()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_build_inclusion_list_filters_out_blob_transactions() {
    let pool = TestPoolBuilder::default();
    let mut factory = MockTransactionFactory::default();

    // Create a blob (EIP-4844) transaction and a non-blob transaction
    let tx_blob = factory.validated(
        MockTransaction::eip4844().with_max_fee(1_000_000_000u128).with_blob_fee(1_000_000_000u128),
    );
    let tx_nonblob = factory.validated(MockTransaction::eip1559().with_max_fee(1_000_000_000u128));

    let _ = pool.add_transaction(TransactionOrigin::External, tx_blob.transaction.clone()).await;
    let _ = pool.add_transaction(TransactionOrigin::External, tx_nonblob.transaction.clone()).await;

    let il = pool.build_inclusion_list(1_000_000_000, usize::MAX / 2);

    assert!(il.len() == 1, "Inclusion list must exclude blob (EIP-4844) transactions");
}
