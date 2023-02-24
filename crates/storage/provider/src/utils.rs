use reth_db::{
    models::{StoredBlockBody, StoredBlockOmmers, StoredBlockWithdrawals},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::{provider::ProviderError, Result};
use reth_primitives::{SealedBlock, TransitionId, U256};

/// Insert block data into corresponding tables. Used mainly for testing & internal tooling.
///
///
/// Check parent dependency in [tables::HeaderNumbers] and in [tables::BlockBodies] tables.
/// Inserts header data to [tables::CanonicalHeaders], [tables::Headers], [tables::HeaderNumbers].
/// and transactions data to [tables::TxSenders], [tables::Transactions], [tables::TxHashNumber].
/// and transition indexes to [tables::BlockTransitionIndex] and [tables::TxTransitionIndex].
/// And block data [tables::BlockBodies], [tables::BlockBodies] and [tables::BlockWithdrawals].
///
/// Return [TransitionId] `(from,to)`
pub fn insert_block<'a, TX: DbTxMut<'a> + DbTx<'a>>(
    tx: &TX,
    block: &SealedBlock,
    has_block_reward: bool,
    parent_tx_num_transition_id: Option<(u64, u64)>,
) -> Result<(TransitionId, TransitionId)> {
    tx.put::<tables::CanonicalHeaders>(block.number, block.hash())?;
    // Put header with canonical hashes.
    tx.put::<tables::Headers>(block.number, block.header.as_ref().clone())?;
    tx.put::<tables::HeaderNumbers>(block.hash(), block.number)?;

    // total difficulty
    let ttd = if block.number == 0 {
        U256::ZERO
    } else {
        let parent_block_number = block.number - 1;
        let parent_ttd = tx.get::<tables::HeaderTD>(parent_block_number)?.unwrap_or_default();
        parent_ttd.0 + block.difficulty
    };

    tx.put::<tables::HeaderTD>(block.number, ttd.into())?;

    // insert body ommers data
    if !block.ommers.is_empty() {
        tx.put::<tables::BlockOmmers>(
            block.number,
            StoredBlockOmmers { ommers: block.ommers.iter().map(|h| h.as_ref().clone()).collect() },
        )?;
    }

    let (mut current_tx_id, mut transition_id) =
        if let Some(parent_tx_num_transition_id) = parent_tx_num_transition_id {
            parent_tx_num_transition_id
        } else if block.number == 0 {
            (0, 0)
        } else {
            let prev_block_num = block.number - 1;
            let prev_body = tx
                .get::<tables::BlockBodies>(prev_block_num)?
                .ok_or(ProviderError::BlockBody { number: prev_block_num })?;
            let last_transition_id = tx
                .get::<tables::BlockTransitionIndex>(prev_block_num)?
                .ok_or(ProviderError::BlockTransition { block_number: prev_block_num })?;
            (prev_body.start_tx_id + prev_body.tx_count, last_transition_id)
        };
    let from_transition = transition_id;
    // insert body data
    tx.put::<tables::BlockBodies>(
        block.number,
        StoredBlockBody { start_tx_id: current_tx_id, tx_count: block.body.len() as u64 },
    )?;

    for transaction in block.body.iter() {
        let rec_tx = transaction.clone().into_ecrecovered().unwrap();
        let hash = rec_tx.hash();
        tx.put::<tables::TxSenders>(current_tx_id, rec_tx.signer())?;
        tx.put::<tables::Transactions>(current_tx_id, rec_tx.into())?;
        tx.put::<tables::TxTransitionIndex>(current_tx_id, transition_id)?;
        tx.put::<tables::TxHashNumber>(hash, current_tx_id)?;
        transition_id += 1;
        current_tx_id += 1;
    }

    let mut has_withdrawals = false;
    if let Some(withdrawals) = block.withdrawals.clone() {
        if !withdrawals.is_empty() {
            has_withdrawals = true;
            tx.put::<tables::BlockWithdrawals>(
                block.number,
                StoredBlockWithdrawals { withdrawals },
            )?;
        }
    }

    if has_block_reward || has_withdrawals {
        transition_id += 1;
    }
    tx.put::<tables::BlockTransitionIndex>(block.number, transition_id)?;

    let to_transition = transition_id;
    Ok((from_transition, to_transition))
}

/// Inserts canonical block in blockchain. Parent tx num and transition id is taken from
/// parent block in database.
pub fn insert_canonical_block<'a, TX: DbTxMut<'a> + DbTx<'a>>(
    tx: &TX,
    block: &SealedBlock,
    has_block_reward: bool,
) -> Result<(TransitionId, TransitionId)> {
    insert_block(tx, block, has_block_reward, None)
}
