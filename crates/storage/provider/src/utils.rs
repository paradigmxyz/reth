use reth_db::{
    cursor::DbCursorRO,
    models::{StoredBlockBodyIndices, StoredBlockOmmers, StoredBlockWithdrawals},
    tables,
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use reth_primitives::{Address, SealedBlock};

/// Insert block data into corresponding tables. Used mainly for testing & internal tooling.
///
///
/// Check parent dependency in [tables::HeaderNumbers] and in [tables::BlockBodyIndices] tables.
/// Inserts header data to [tables::CanonicalHeaders], [tables::Headers], [tables::HeaderNumbers].
/// and transactions data to [tables::TxSenders], [tables::Transactions], [tables::TxHashNumber].
/// and transition/transaction meta data to [tables::BlockBodyIndices]
/// and block data to [tables::BlockOmmers] and [tables::BlockWithdrawals].
///
/// Return [StoredBlockBodyIndices] that contains indices of the first and last transactions and
/// transition in the block.
pub fn insert_block<'a, TX: DbTxMut<'a> + DbTx<'a>>(
    tx: &TX,
    block: SealedBlock,
    senders: Option<Vec<Address>>,
) -> Result<StoredBlockBodyIndices, DatabaseError> {
    let block_number = block.number;
    tx.put::<tables::CanonicalHeaders>(block.number, block.hash())?;
    // Put header with canonical hashes.
    tx.put::<tables::Headers>(block.number, block.header.as_ref().clone())?;
    tx.put::<tables::HeaderNumbers>(block.hash(), block.number)?;

    // total difficulty
    let ttd = if block.number == 0 {
        block.difficulty
    } else {
        let parent_block_number = block.number - 1;
        let parent_ttd = tx.get::<tables::HeaderTD>(parent_block_number)?.unwrap_or_default();
        parent_ttd.0 + block.difficulty
    };

    tx.put::<tables::HeaderTD>(block.number, ttd.into())?;

    // insert body ommers data
    if !block.ommers.is_empty() {
        tx.put::<tables::BlockOmmers>(block.number, StoredBlockOmmers { ommers: block.ommers })?;
    }

    let mut next_tx_num =
        tx.cursor_read::<tables::Transactions>()?.last()?.map(|(n, _)| n + 1).unwrap_or_default();
    let first_tx_num = next_tx_num;

    let tx_count = block.body.len() as u64;

    let senders_len = senders.as_ref().map(|s| s.len());
    let tx_iter = if Some(block.body.len()) == senders_len {
        block.body.into_iter().zip(senders.unwrap().into_iter()).collect::<Vec<(_, _)>>()
    } else {
        block
            .body
            .into_iter()
            .map(|tx| {
                let signer = tx.recover_signer();
                (tx, signer.unwrap_or_default())
            })
            .collect::<Vec<(_, _)>>()
    };

    for (transaction, sender) in tx_iter {
        let hash = transaction.hash();
        tx.put::<tables::TxSenders>(next_tx_num, sender)?;
        tx.put::<tables::Transactions>(next_tx_num, transaction.into())?;
        tx.put::<tables::TxHashNumber>(hash, next_tx_num)?;
        next_tx_num += 1;
    }

    if let Some(withdrawals) = block.withdrawals {
        if !withdrawals.is_empty() {
            tx.put::<tables::BlockWithdrawals>(
                block_number,
                StoredBlockWithdrawals { withdrawals },
            )?;
        }
    }

    let block_indices = StoredBlockBodyIndices { first_tx_num, tx_count };
    tx.put::<tables::BlockBodyIndices>(block_number, block_indices.clone())?;

    if !block_indices.is_empty() {
        tx.put::<tables::TransactionBlock>(block_indices.last_tx_num(), block_number)?;
    }

    Ok(block_indices)
}

/// Inserts canonical block in blockchain. Parent tx num and transition id is taken from
/// parent block in database.
pub fn insert_canonical_block<'a, TX: DbTxMut<'a> + DbTx<'a>>(
    tx: &TX,
    block: SealedBlock,
    senders: Option<Vec<Address>>,
) -> Result<StoredBlockBodyIndices, DatabaseError> {
    insert_block(tx, block, senders)
}
