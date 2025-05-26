//! Tests for the transaction pool maintenance components

use super::*;
use crate::{
    blobstore::InMemoryBlobStore, metrics::MaintainPoolMetrics,
    validate::EthTransactionValidatorBuilder, CoinbaseTipOrdering, EthPooledTransaction, Pool,
    TransactionOrigin,
};
use alloy_eips::eip2718::Decodable2718;
use alloy_primitives::{hex, Address, U256};
use reth_ethereum_primitives::{PooledTransactionVariant, TransactionSigned};
use reth_fs_util as fs;
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_tasks::TaskManager;

#[test]
fn test_drift_monitor_basic() {
    let metrics = MaintainPoolMetrics::default();
    let mut monitor = DriftMonitor::new(10, metrics);
    assert_eq!(monitor.state(), PoolDriftState::InSync);

    // Setting state to drifted
    monitor.set_state(PoolDriftState::Drifted);
    assert_eq!(monitor.state(), PoolDriftState::Drifted);
    assert!(monitor.state().is_drifted());

    // Setting state back to InSync
    monitor.set_state(PoolDriftState::InSync);
    assert_eq!(monitor.state(), PoolDriftState::InSync);
    assert!(!monitor.state().is_drifted());
}

#[test]
fn test_drift_monitor_dirty_addresses() {
    let metrics = MaintainPoolMetrics::default();
    let mut monitor = DriftMonitor::new(10, metrics);

    assert_eq!(monitor.dirty_address_count(), 0);

    // Add addresses
    let addr1 = Address::random();
    let addr2 = Address::random();
    monitor.add_dirty_addresses([addr1, addr2]);

    assert_eq!(monitor.dirty_address_count(), 2);
    assert!(monitor.has_dirty_addresses());

    // Remove addresses
    monitor.remove_dirty_address(&addr1);
    assert_eq!(monitor.dirty_address_count(), 1);

    // Take addresses
    let addresses = monitor.take_dirty_addresses();
    assert_eq!(addresses.len(), 1);
    assert!(addresses.contains(&addr2));
    assert_eq!(monitor.dirty_address_count(), 0);
    assert!(!monitor.has_dirty_addresses());
}

#[test]
fn test_first_event_handling() {
    let metrics = MaintainPoolMetrics::default();
    let mut monitor = DriftMonitor::new(10, metrics);

    assert!(!monitor.state().is_drifted());

    // First event should set to drifted
    monitor.on_first_event();
    assert!(monitor.state().is_drifted());

    // Reset state
    monitor.set_state(PoolDriftState::InSync);

    // Second call should have no effect since first_event is now false
    monitor.on_first_event();
    assert!(!monitor.state().is_drifted());
}

#[test]
fn test_finalized_block_tracker() {
    // Test with higher finalized block
    let mut tracker = FinalizedBlockTracker::new(Some(10));
    assert_eq!(tracker.update(Some(15)), Some(15));

    // Test with lower finalized block
    let mut tracker = FinalizedBlockTracker::new(Some(20));
    assert_eq!(tracker.update(Some(15)), None);

    // Test with equal finalized block
    let mut tracker = FinalizedBlockTracker::new(Some(20));
    assert_eq!(tracker.update(Some(20)), None);

    // Test with no previous finalized block
    let mut tracker = FinalizedBlockTracker::new(None);
    assert_eq!(tracker.update(Some(10)), Some(10));

    // Test with no new finalized block
    let mut tracker = FinalizedBlockTracker::new(Some(10));
    assert_eq!(tracker.update(None), None);

    // Test with no finalized blocks
    let mut tracker = FinalizedBlockTracker::new(None);
    assert_eq!(tracker.update(None), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_save_local_txs_backup() {
    let temp_dir = tempfile::tempdir().unwrap();
    let transactions_path = temp_dir.path().join("test_transactions_backup").with_extension("rlp");
    let tx_bytes = hex!(
        "02f87201830655c2808505ef61f08482565f94388c818ca8b9251b393131c08a736a67ccb192978801049e39c4b5b1f580c001a01764ace353514e8abdfb92446de356b260e3c1225b73fc4c8876a6258d12a129a04f02294aa61ca7676061cd99f29275491218b4754b46a0248e5e42bc5091f507"
    );
    let tx = PooledTransactionVariant::decode_2718(&mut &tx_bytes[..]).unwrap();
    let provider = MockEthProvider::default();
    let transaction = EthPooledTransaction::from_pooled(tx.try_into_recovered().unwrap());
    let tx_to_cmp = transaction.clone();
    let sender = hex!("1f9090aaE28b8a3dCeaDf281B0F12828e676c326").into();
    provider.add_account(sender, ExtendedAccount::new(42, U256::MAX));
    let blob_store = InMemoryBlobStore::default();
    let validator = EthTransactionValidatorBuilder::new(provider).build(blob_store.clone());

    let txpool = Pool::new(
        validator.clone(),
        CoinbaseTipOrdering::default(),
        blob_store.clone(),
        Default::default(),
    );

    txpool.add_transaction(TransactionOrigin::Local, transaction.clone()).await.unwrap();

    let handle = tokio::runtime::Handle::current();
    let manager = TaskManager::new(handle);
    let config = LocalTransactionBackupConfig::with_local_txs_backup(transactions_path.clone());
    manager.executor().spawn_critical_with_graceful_shutdown_signal("test task", |shutdown| {
        backup_local_transactions_task(shutdown, txpool.clone(), config)
    });

    let mut txns = txpool.get_local_transactions();
    let tx_on_finish = txns.pop().expect("there should be 1 transaction");

    assert_eq!(*tx_to_cmp.hash(), *tx_on_finish.hash());

    // shutdown the executor
    manager.graceful_shutdown();

    let data = fs::read(transactions_path).unwrap();

    let txs: Vec<TransactionSigned> = alloy_rlp::Decodable::decode(&mut data.as_slice()).unwrap();
    assert_eq!(txs.len(), 1);

    temp_dir.close().unwrap();
}
