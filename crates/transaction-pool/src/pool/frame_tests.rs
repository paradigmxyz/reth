//! Regressions for admission and canonical updates while frames are withdrawn.

use super::*;
use crate::{
    blobstore::InMemoryBlobStore, test_utils::OkValidator, validate::FrameValidation,
    CoinbaseTipOrdering, EthPooledTransaction, PoolUpdateKind, U256,
};
use alloy_consensus::{BlockBody, Header, Transaction, TxEip8141};
use alloy_eips::eip8141::{Frame, TransactionFees};
use alloy_primitives::Sealable;
use reth_ethereum_primitives::{Block, TransactionSigned};
use reth_primitives_traits::{SealedBlock, SealedHeader};

type FramePool =
    PoolInner<OkValidator, CoinbaseTipOrdering<EthPooledTransaction>, InMemoryBlobStore>;

fn pool() -> FramePool {
    PoolInner::new(
        OkValidator::default(),
        CoinbaseTipOrdering::default(),
        InMemoryBlobStore::default(),
        PoolConfig::default(),
    )
}

fn frame(sender: Address, nonce: u64, fee: u64, tip: u64, blob: bool) -> EthPooledTransaction {
    frame_with_blob_fee(sender, nonce, fee, tip, blob.then_some(100))
}

fn frame_with_blob_fee(
    sender: Address,
    nonce: u64,
    fee: u64,
    tip: u64,
    blob_fee: Option<u64>,
) -> EthPooledTransaction {
    let tx = TxEip8141 {
        chain_id: 1,
        sender,
        nonce,
        frames: vec![Frame::default()],
        fees: TransactionFees {
            max_priority_fee_per_gas: U256::from(tip),
            max_fee_per_gas: U256::from(fee),
            max_fee_per_blob_gas: U256::from(blob_fee.unwrap_or_default()),
        },
        blob_versioned_hashes: if blob_fee.is_some() {
            vec![B256::repeat_byte(1)]
        } else {
            Vec::new()
        },
        ..Default::default()
    };
    let mut tx = EthPooledTransaction::new(
        alloy_consensus::transaction::Recovered::new_unchecked(
            TransactionSigned::Eip8141(tx.seal_slow()),
            sender,
        ),
        0,
    );
    tx.set_frame_validation(Arc::new(FrameValidation {
        sender,
        sender_nonce: nonce,
        state_nonce: 0,
        sender_balance: U256::from(100),
        sender_code_hash: None,
        payer: sender,
        max_cost: U256::from(5),
        payer_balance: U256::from(100),
        head_hash: B256::ZERO,
        dependencies: Default::default(),
        expires_at: None,
        exclusive_payer: false,
    }))
    .unwrap();
    tx
}

fn withdraw(
    pool: &FramePool,
    tx: EthPooledTransaction,
) -> Arc<ValidPoolTransaction<EthPooledTransaction>> {
    let tx = Arc::new(ValidPoolTransaction {
        transaction_id: TransactionId::new(pool.get_sender_id(tx.sender()), tx.nonce()),
        transaction: tx,
        propagate: true,
        timestamp: Instant::now(),
        origin: TransactionOrigin::External,
        authority_ids: None,
    });
    pool.frame_revalidation.lock().insert(Arc::clone(&tx));
    tx
}

fn admit(pool: &FramePool, tx: EthPooledTransaction) -> PoolResult<AddedTransactionOutcome> {
    pool.add_transaction(
        &mut pool.pool.write(),
        TransactionOrigin::External,
        TransactionValidationOutcome::Valid {
            balance: U256::from(100),
            state_nonce: 0,
            transaction: ValidTransaction::Valid(tx),
            propagate: true,
            bytecode_hash: None,
            authorities: None,
        },
    )
    .0
}

#[test]
fn withdrawn_frame_replacements_require_both_fee_bumps() {
    for (fee, tip) in [(99, 11), (110, 9), (109, 11), (110, 10)] {
        let pool = pool();
        let sender = Address::repeat_byte(1);
        let old = withdraw(&pool, frame(sender, 0, 100, 10, false));
        assert!(matches!(
            admit(&pool, frame(sender, 0, fee, tip, false)),
            Err(PoolError { kind: PoolErrorKind::ReplacementUnderpriced, .. })
        ));
        assert!(pool.frame_revalidation.lock().take_current(&old));
    }
    let pool = pool();
    let sender = Address::repeat_byte(1);
    let old = withdraw(&pool, frame(sender, 0, 100, 10, false));
    assert!(admit(&pool, frame(sender, 0, 110, 11, false)).is_ok());
    assert!(!pool.frame_revalidation.lock().take_current(&old));
}

#[test]
fn admitting_another_nonce_does_not_cancel_withdrawn_frame() {
    let pool = pool();
    let sender = Address::repeat_byte(2);
    let old = withdraw(&pool, frame(sender, 0, 100, 10, false));
    assert!(admit(&pool, frame(sender, 1, 100, 10, false)).is_ok());
    assert!(pool.frame_revalidation.lock().take_current(&old));
}

#[test]
fn withdrawn_blob_frame_replacements_require_blob_fee_bump() {
    let pool = pool();
    let sender = Address::repeat_byte(6);
    let old = withdraw(&pool, frame(sender, 0, 100, 10, true));
    assert!(matches!(
        admit(&pool, frame_with_blob_fee(sender, 0, 200, 20, Some(199))),
        Err(PoolError { kind: PoolErrorKind::ReplacementUnderpriced, .. })
    ));
    assert!(pool.frame_revalidation.lock().get(sender, 0).is_some());
    assert!(admit(&pool, frame_with_blob_fee(sender, 0, 200, 20, Some(200))).is_ok());
    assert!(!pool.frame_revalidation.lock().take_current(&old));
}

#[test]
fn failed_replacement_admission_preserves_withdrawn_frame() {
    let pool = pool();
    let sender = Address::repeat_byte(7);
    let old = withdraw(&pool, frame(sender, 0, 100, 10, false));
    let mut replacement = frame(sender, 0, 110, 11, false);
    Arc::make_mut(replacement.frame_validation.as_mut().unwrap()).head_hash = B256::repeat_byte(8);
    assert!(admit(&pool, replacement).is_err());
    assert!(pool.frame_revalidation.lock().take_current(&old));
}

#[test]
fn descendant_removal_preserves_withdrawn_ancestors() {
    let pool = pool();
    let sender = Address::repeat_byte(3);
    let ancestor = withdraw(&pool, frame(sender, 0, 100, 10, false));
    let selected = withdraw(&pool, frame(sender, 1, 100, 10, false));
    let descendant = withdraw(&pool, frame(sender, 2, 100, 10, false));
    let removed = pool.remove_transactions_and_descendants(vec![*selected.hash()]);
    assert_eq!(removed.len(), 2);
    let mut queue = pool.frame_revalidation.lock();
    assert!(queue.take_current(&ancestor));
    assert!(!queue.take_current(&selected));
    assert!(!queue.take_current(&descendant));
}

#[test]
fn new_head_with_no_account_changes_withdraws_blob_frames() {
    let pool = pool();
    let blob = frame(Address::repeat_byte(4), 0, 100, 10, true);
    let ordinary = frame(Address::repeat_byte(5), 0, 100, 10, false);
    let blob_hash = *blob.hash();
    let ordinary_hash = *ordinary.hash();
    admit(&pool, blob).unwrap();
    admit(&pool, ordinary).unwrap();
    assert_eq!(pool.pool.read().blob_frame_transaction_hashes(), vec![blob_hash]);
    let tip = SealedBlock::<Block>::from_sealed_parts(
        SealedHeader::new(
            Header { gas_limit: 30_000_000, ..Default::default() },
            B256::repeat_byte(9),
        ),
        BlockBody::default(),
    );
    pool.on_canonical_state_change(CanonicalStateUpdate {
        new_tip: &tip,
        pending_block_base_fee: 0,
        pending_block_blob_fee: Some(2),
        changed_accounts: Vec::new(),
        mined_transactions: Vec::new(),
        update_kind: PoolUpdateKind::Commit,
    });
    assert!(pool.get(&blob_hash).is_none());
    assert!(pool.get(&ordinary_hash).is_some());
    let snapshot = pool.frame_revalidation.lock().snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(*snapshot[0].hash(), blob_hash);
}
