use assert_matches::assert_matches;
use reth_transaction_pool::{
    test_utils::{testing_pool, MockTransactionFactory},
    FullTransactionEvent, TransactionEvent, TransactionOrigin, TransactionPool,
};
use tokio_stream::StreamExt;

#[tokio::test(flavor = "multi_thread")]
async fn txpool_listener_by_hash() {
    let txpool = testing_pool();
    let mut mock_tx_factory = MockTransactionFactory::default();
    let transaction = mock_tx_factory.create_eip1559();

    let result = txpool
        .add_transaction_and_subscribe(TransactionOrigin::External, transaction.transaction.clone())
        .await;
    assert_matches!(result, Ok(_));

    let mut events = result.unwrap();
    assert_matches!(events.next().await, Some(TransactionEvent::Pending));
}

#[tokio::test(flavor = "multi_thread")]
async fn txpool_listener_all() {
    let txpool = testing_pool();
    let mut mock_tx_factory = MockTransactionFactory::default();
    let transaction = mock_tx_factory.create_eip1559();

    let mut all_tx_events = txpool.all_transactions_event_listener();

    let added_result =
        txpool.add_transaction(TransactionOrigin::External, transaction.transaction.clone()).await;
    assert_matches!(added_result, Ok(hash) if hash == transaction.transaction.get_hash());

    assert_matches!(
        all_tx_events.next().await,
        Some(FullTransactionEvent::Pending(hash)) if hash == transaction.transaction.get_hash()
    );
}
