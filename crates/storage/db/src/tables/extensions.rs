use itertools::Itertools;
use std::{ops::Range, sync::mpsc};

use reth_interfaces::{db::DatabaseError, RethError, RethResult};
use reth_primitives::{keccak256, TransactionSignedNoHash, TxHash, TxNumber, B256};

use crate::{abstraction::cursor::DbCursorRO, transaction::DbTx, Transactions};

impl Transactions {
    /// Recovers transaction hashes by walking through [`tables::Transactions`] table and
    /// calculating them in a parallel manner. Returned unsorted.
    pub fn recover_hashes<'a, 'b, TX: DbTx<'a>>(
        tx: &'b TX,
        tx_range: Range<TxNumber>,
    ) -> RethResult<Vec<(TxHash, TxNumber)>>
    where
        'a: 'b,
    {
        let mut tx_cursor = tx.cursor_read::<Self>()?;
        let tx_range_size = tx_range.clone().count();
        let tx_walker = tx_cursor.walk_range(tx_range)?;

        let chunk_size = (tx_range_size / rayon::current_num_threads()).max(1);
        let mut channels = Vec::with_capacity(chunk_size);
        let mut transaction_count = 0;

        for chunk in &tx_walker.chunks(chunk_size) {
            let (tx, rx) = mpsc::channel();
            channels.push(rx);

            // Note: Unfortunate side-effect of how chunk is designed in itertools (it is not Send)
            let chunk: Vec<_> = chunk.collect();
            transaction_count += chunk.len();

            // Spawn the task onto the global rayon pool
            // This task will send the results through the channel after it has calculated the hash.
            rayon::spawn(move || {
                let mut rlp_buf = Vec::with_capacity(128);
                for entry in chunk {
                    rlp_buf.clear();
                    let _ = tx.send(calculate_hash(entry, &mut rlp_buf));
                }
            });
        }
        let mut tx_list = Vec::with_capacity(transaction_count);

        // Iterate over channels and append the tx hashes to be sorted out later
        for channel in channels {
            while let Ok(tx) = channel.recv() {
                let (tx_hash, tx_id) = tx.map_err(|boxed| *boxed)?;
                tx_list.push((tx_hash, tx_id));
            }
        }

        Ok(tx_list)
    }
}

/// Calculates the hash of the given transaction
#[inline]
fn calculate_hash(
    entry: Result<(TxNumber, TransactionSignedNoHash), DatabaseError>,
    rlp_buf: &mut Vec<u8>,
) -> Result<(B256, TxNumber), Box<RethError>> {
    let (tx_id, tx) = entry.map_err(|e| Box::new(e.into()))?;
    tx.transaction.encode_with_signature(&tx.signature, rlp_buf, false);
    Ok((keccak256(rlp_buf), tx_id))
}
