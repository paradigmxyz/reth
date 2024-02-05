use assert_matches::assert_matches;
use reth_transaction_pool::{
    noop::MockTransactionValidator,
    test_utils::{MockTransactionFactory, TestPoolBuilder},
    FullTransactionEvent, TransactionEvent, TransactionListenerKind, TransactionOrigin,
    TransactionPool,
};
use std::{future::poll_fn, task::Poll};
use tokio_stream::StreamExt;

#[tokio::test(flavor = "multi_thread")]
async fn txpool_listener_by_hash() {
    let txpool = TestPoolBuilder::default();
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
    let txpool = TestPoolBuilder::default();
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

#[tokio::test(flavor = "multi_thread")]
async fn txpool_listener_propagate_only() {
    let txpool =
        TestPoolBuilder::default().with_validator(MockTransactionValidator::no_propagate_local());
    let mut mock_tx_factory = MockTransactionFactory::default();
    let transaction = mock_tx_factory.create_eip1559();
    let expected = *transaction.hash();
    let mut listener_network = txpool.pending_transactions_listener();
    let mut listener_all = txpool.pending_transactions_listener_for(TransactionListenerKind::All);
    let result =
        txpool.add_transaction(TransactionOrigin::Local, transaction.transaction.clone()).await;
    assert!(result.is_ok());

    let inserted = listener_all.recv().await.unwrap();
    assert_eq!(inserted, expected);

    poll_fn(|cx| {
        // no propagation
        assert!(listener_network.poll_recv(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn txpool_listener_new_propagate_only() {
    let txpool =
        TestPoolBuilder::default().with_validator(MockTransactionValidator::no_propagate_local());
    let mut mock_tx_factory = MockTransactionFactory::default();
    let transaction = mock_tx_factory.create_eip1559();
    let expected = *transaction.hash();
    let mut listener_network = txpool.new_transactions_listener();
    let mut listener_all = txpool.new_transactions_listener_for(TransactionListenerKind::All);
    let result =
        txpool.add_transaction(TransactionOrigin::Local, transaction.transaction.clone()).await;
    assert!(result.is_ok());

    let inserted = listener_all.recv().await.unwrap();
    let actual = *inserted.transaction.hash();
    assert_eq!(actual, expected);

    poll_fn(|cx| {
        // no propagation
        assert!(listener_network.poll_recv(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}
