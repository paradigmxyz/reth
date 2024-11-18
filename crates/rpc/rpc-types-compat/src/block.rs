//! Compatibility functions for rpc `Block` type.

use alloy_consensus::Sealed;
use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use alloy_rpc_types_eth::{
    Block, BlockTransactions, BlockTransactionsKind, Header, TransactionInfo,
};
use reth_primitives::{Block as PrimitiveBlock, BlockWithSenders};

use crate::{transaction::from_recovered_with_block_context, TransactionCompat};

/// Converts the given primitive block into a [`Block`] response with the given
/// [`BlockTransactionsKind`]
///
/// If a `block_hash` is provided, then this is used, otherwise the block hash is computed.
pub fn from_block<T: TransactionCompat>(
    block: BlockWithSenders,
    total_difficulty: U256,
    kind: BlockTransactionsKind,
    block_hash: Option<B256>,
    tx_resp_builder: &T,
) -> Result<Block<T::Transaction>, T::Error> {
    match kind {
        BlockTransactionsKind::Hashes => {
            Ok(from_block_with_tx_hashes::<T::Transaction>(block, total_difficulty, block_hash))
        }
        BlockTransactionsKind::Full => {
            from_block_full::<T>(block, total_difficulty, block_hash, tx_resp_builder)
        }
    }
}

/// Create a new [`Block`] response from a [primitive block](reth_primitives::Block), using the
/// total difficulty to populate its field in the rpc response.
///
/// This will populate the `transactions` field with only the hashes of the transactions in the
/// block: [`BlockTransactions::Hashes`]
pub fn from_block_with_tx_hashes<T>(
    block: BlockWithSenders,
    total_difficulty: U256,
    block_hash: Option<B256>,
) -> Block<T> {
    let block_hash = block_hash.unwrap_or_else(|| block.header.hash_slow());
    let transactions = block.body.transactions().map(|tx| tx.hash()).collect();

    from_block_with_transactions(
        block.length(),
        block_hash,
        block.block,
        total_difficulty,
        BlockTransactions::Hashes(transactions),
    )
}

/// Create a new [`Block`] response from a [primitive block](reth_primitives::Block), using the
/// total difficulty to populate its field in the rpc response.
///
/// This will populate the `transactions` field with the _full_
/// [`TransactionCompat::Transaction`] objects: [`BlockTransactions::Full`]
pub fn from_block_full<T: TransactionCompat>(
    mut block: BlockWithSenders,
    total_difficulty: U256,
    block_hash: Option<B256>,
    tx_resp_builder: &T,
) -> Result<Block<T::Transaction>, T::Error> {
    let block_hash = block_hash.unwrap_or_else(|| block.block.header.hash_slow());
    let block_number = block.block.number;
    let base_fee_per_gas = block.block.base_fee_per_gas;

    // NOTE: we can safely remove the body here because not needed to finalize the `Block` in
    // `from_block_with_transactions`, however we need to compute the length before
    let block_length = block.block.length();
    let transactions = std::mem::take(&mut block.block.body.transactions);
    let transactions_with_senders = transactions.into_iter().zip(block.senders);
    let transactions = transactions_with_senders
        .enumerate()
        .map(|(idx, (tx, sender))| {
            let tx_hash = tx.hash();
            let signed_tx_ec_recovered = tx.with_signer(sender);
            let tx_info = TransactionInfo {
                hash: Some(tx_hash),
                block_hash: Some(block_hash),
                block_number: Some(block_number),
                base_fee: base_fee_per_gas.map(u128::from),
                index: Some(idx as u64),
            };

            from_recovered_with_block_context::<T>(signed_tx_ec_recovered, tx_info, tx_resp_builder)
        })
        .collect::<Result<Vec<_>, T::Error>>()?;

    Ok(from_block_with_transactions(
        block_length,
        block_hash,
        block.block,
        total_difficulty,
        BlockTransactions::Full(transactions),
    ))
}

#[inline]
fn from_block_with_transactions<T>(
    block_length: usize,
    block_hash: B256,
    block: PrimitiveBlock,
    total_difficulty: U256,
    transactions: BlockTransactions<T>,
) -> Block<T> {
    let withdrawals = block
        .header
        .withdrawals_root
        .is_some()
        .then(|| block.body.withdrawals.map(Withdrawals::into_inner).map(Into::into))
        .flatten();

    let uncles = block.body.ommers.into_iter().map(|h| h.hash_slow()).collect();
    let header = Header::from_consensus(
        Sealed::new_unchecked(block.header, block_hash),
        Some(total_difficulty),
        Some(U256::from(block_length)),
    );

    Block { header, uncles, transactions, withdrawals }
}
