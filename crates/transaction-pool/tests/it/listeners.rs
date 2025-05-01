use assert_matches::assert_matches;
use reth_transaction_pool::{
    noop::MockTransactionValidator,
    test_utils::{MockTransactionFactory, TestPoolBuilder},
    FullTransactionEvent, PoolTransaction, TransactionEvent, TransactionListenerKind,
    TransactionOrigin, TransactionPool,
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

    let removed_txs = txpool.remove_transactions(vec![*transaction.transaction.hash()]);
    assert_eq!(transaction.transaction.hash(), removed_txs[0].transaction.hash());

    assert_matches!(events.next().await, Some(TransactionEvent::Discarded));
}

#[tokio::test(flavor = "multi_thread")]
async fn txpool_listener_replace_event() {
    let txpool = TestPoolBuilder::default();
    let mut mock_tx_factory = MockTransactionFactory::default();
    let transaction = mock_tx_factory.create_eip1559();

    let mut all_tx_events = txpool.all_transactions_event_listener();

    let old_transaction = transaction.transaction.clone();
    let mut result = txpool
        .add_transaction_and_subscribe(TransactionOrigin::External, old_transaction.clone())
        .await;
    assert_matches!(result, Ok(_));

    let mut events = result.unwrap();
    assert_matches!(events.next().await, Some(TransactionEvent::Pending));
    assert_matches!(all_tx_events.next().await, Some(FullTransactionEvent::Pending(hash)) if hash == *old_transaction.get_hash());

    // add replace tx.
    let replace_transaction = transaction.transaction.clone().rng_hash().inc_price();
    result = txpool
        .add_transaction_and_subscribe(TransactionOrigin::External, replace_transaction.clone())
        .await;
    assert_matches!(result, Ok(_));

    let mut new_events = result.unwrap();
    assert_matches!(new_events.next().await, Some(TransactionEvent::Pending));

    // The listener of old transaction should receive replaced event.
    assert_matches!(events.next().await, Some(TransactionEvent::Replaced(hash)) if hash == *replace_transaction.get_hash());

    // The listener of all should receive one pending event of new transaction and one replaced
    // event of old transaction.
    assert_matches!(all_tx_events.next().await, Some(FullTransactionEvent::Pending(hash)) if hash == *replace_transaction.get_hash());
    assert_matches!(all_tx_events.next().await, Some(FullTransactionEvent::Replaced { transaction, replaced_by }) if *transaction.transaction.get_hash() == *old_transaction.get_hash() && replaced_by == *replace_transaction.get_hash());
}

#[tokio::test(flavor = "multi_thread")]
async fn txpool_listener_queued_event() {
    let txpool = TestPoolBuilder::default();
    let mut mock_tx_factory = MockTransactionFactory::default();
    let transaction = mock_tx_factory.create_eip1559().transaction.inc_nonce();

    let mut all_tx_events = txpool.all_transactions_event_listener();

    let result = txpool
        .add_transaction_and_subscribe(TransactionOrigin::External, transaction.clone())
        .await;
    assert_matches!(result, Ok(_));

    let mut events = result.unwrap();
    assert_matches!(events.next().await, Some(TransactionEvent::Queued));

    // The listener of all should receive queued event as well.
    assert_matches!(all_tx_events.next().await, Some(FullTransactionEvent::Queued(hash)) if hash == *transaction.get_hash());
}

#[tokio::test(flavor = "multi_thread")]
async fn txpool_listener_invalid_event() {
    let txpool =
        TestPoolBuilder::default().with_validator(MockTransactionValidator::return_invalid());
    let mut mock_tx_factory = MockTransactionFactory::default();
    let transaction = mock_tx_factory.create_eip1559().transaction;

    let mut all_tx_events = txpool.all_transactions_event_listener();

    let result = txpool
        .add_transaction_and_subscribe(TransactionOrigin::External, transaction.clone())
        .await;
    assert_matches!(result, Err(_));

    // The listener of all should receive invalid event.
    assert_matches!(all_tx_events.next().await, Some(FullTransactionEvent::Invalid(hash)) if hash == *transaction.get_hash());
}

#[tokio::test(flavor = "multi_thread")]
async fn txpool_listener_all() {
    let txpool = TestPoolBuilder::default();
    let mut mock_tx_factory = MockTransactionFactory::default();
    let transaction = mock_tx_factory.create_eip1559();

    let mut all_tx_events = txpool.all_transactions_event_listener();

    let added_result =
        txpool.add_transaction(TransactionOrigin::External, transaction.transaction.clone()).await;
    assert_matches!(added_result, Ok(hash) if hash == *transaction.transaction.get_hash());

    assert_matches!(
        all_tx_events.next().await,
        Some(FullTransactionEvent::Pending(hash)) if hash == *transaction.transaction.get_hash()
    );

    let removed_txs = txpool.remove_transactions(vec![*transaction.transaction.hash()]);

    assert_eq!(transaction.transaction.hash(), removed_txs[0].transaction.hash());

    assert_matches!(all_tx_events.next().await, Some(FullTransactionEvent::Discarded(hash)) if hash == *transaction.transaction.get_hash());
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

#[tokio::test(flavor = "multi_thread")]
async fn txpool_listener_blob_sidecar() {
    let txpool =
        TestPoolBuilder::default().with_validator(MockTransactionValidator::no_propagate_local());
    let mut mock_tx_factory = MockTransactionFactory::default();
    let blob_transaction = mock_tx_factory.create_eip4844();
    let expected = *blob_transaction.hash();
    let mut listener_blob = txpool.blob_transaction_sidecars_listener();
    let result = txpool
        .add_transaction(TransactionOrigin::Local, blob_transaction.transaction.clone())
        .await;
    assert!(result.is_ok());

    let inserted = listener_blob.recv().await.unwrap();
    assert_eq!(*inserted.tx_hash, expected);
}
