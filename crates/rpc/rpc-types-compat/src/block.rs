//! Compatibility functions for rpc `Block` type.

use alloy_consensus::{BlockHeader, Sealable, Sealed};
use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::{B256, U256};
use alloy_rpc_types_eth::{
    Block, BlockTransactions, BlockTransactionsKind, Header, TransactionInfo,
};
use reth_primitives::{transaction::SignedTransactionIntoRecoveredExt, BlockWithSenders};
use reth_primitives_traits::{Block as BlockTrait, BlockBody, SignedTransaction};

use crate::transaction::TransactionCompat;

/// Converts the given primitive block into a [`Block`] response with the given
/// [`BlockTransactionsKind`]
///
/// If a `block_hash` is provided, then this is used, otherwise the block hash is computed.
#[expect(clippy::type_complexity)]
pub fn from_block<T, B>(
    block: BlockWithSenders<B>,
    kind: BlockTransactionsKind,
    block_hash: Option<B256>,
    tx_resp_builder: &T,
) -> Result<Block<T::Transaction, Header<B::Header>>, T::Error>
where
    T: TransactionCompat<<<B as BlockTrait>::Body as BlockBody>::Transaction>,
    B: BlockTrait,
{
    match kind {
        BlockTransactionsKind::Hashes => {
            Ok(from_block_with_tx_hashes::<T::Transaction, B>(block, block_hash))
        }
        BlockTransactionsKind::Full => from_block_full::<T, B>(block, block_hash, tx_resp_builder),
    }
}

/// Create a new [`Block`] response from a [primitive block](reth_primitives::Block), using the
/// total difficulty to populate its field in the rpc response.
///
/// This will populate the `transactions` field with only the hashes of the transactions in the
/// block: [`BlockTransactions::Hashes`]
pub fn from_block_with_tx_hashes<T, B>(
    block: BlockWithSenders<B>,
    block_hash: Option<B256>,
) -> Block<T, Header<B::Header>>
where
    B: BlockTrait,
{
    let block_hash = block_hash.unwrap_or_else(|| block.header().hash_slow());
    let transactions = block.body().transaction_hashes_iter().copied().collect();

    from_block_with_transactions(
        block.length(),
        block_hash,
        block.block,
        BlockTransactions::Hashes(transactions),
    )
}

/// Create a new [`Block`] response from a [primitive block](reth_primitives::Block), using the
/// total difficulty to populate its field in the rpc response.
///
/// This will populate the `transactions` field with the _full_
/// [`TransactionCompat::Transaction`] objects: [`BlockTransactions::Full`]
#[expect(clippy::type_complexity)]
pub fn from_block_full<T, B>(
    block: BlockWithSenders<B>,
    block_hash: Option<B256>,
    tx_resp_builder: &T,
) -> Result<Block<T::Transaction, Header<B::Header>>, T::Error>
where
    T: TransactionCompat<<<B as BlockTrait>::Body as BlockBody>::Transaction>,
    B: BlockTrait,
{
    let block_hash = block_hash.unwrap_or_else(|| block.block.header().hash_slow());
    let block_number = block.block.header().number();
    let base_fee_per_gas = block.block.header().base_fee_per_gas();

    // NOTE: we can safely remove the body here because not needed to finalize the `Block` in
    // `from_block_with_transactions`, however we need to compute the length before
    let block_length = block.block.length();
    let transactions = block.block.body().transactions().to_vec();
    let transactions_with_senders = transactions.into_iter().zip(block.senders);
    let transactions = transactions_with_senders
        .enumerate()
        .map(|(idx, (tx, sender))| {
            let tx_hash = *tx.tx_hash();
            let signed_tx_ec_recovered = tx.with_signer(sender);
            let tx_info = TransactionInfo {
                hash: Some(tx_hash),
                block_hash: Some(block_hash),
                block_number: Some(block_number),
                base_fee: base_fee_per_gas.map(u128::from),
                index: Some(idx as u64),
            };

            tx_resp_builder.fill(signed_tx_ec_recovered, tx_info)
        })
        .collect::<Result<Vec<_>, T::Error>>()?;

    Ok(from_block_with_transactions(
        block_length,
        block_hash,
        block.block,
        BlockTransactions::Full(transactions),
    ))
}

#[inline]
fn from_block_with_transactions<T, B: BlockTrait>(
    block_length: usize,
    block_hash: B256,
    block: B,
    transactions: BlockTransactions<T>,
) -> Block<T, Header<B::Header>> {
    let withdrawals = block
        .header()
        .withdrawals_root()
        .is_some()
        .then(|| block.body().withdrawals().cloned().map(Withdrawals::into_inner).map(Into::into))
        .flatten();

    let uncles = block
        .body()
        .ommers()
        .map(|o| o.iter().map(|h| h.hash_slow()).collect())
        .unwrap_or_default();
    let (header, _) = block.split();
    let header = Header::from_consensus(
        Sealed::new_unchecked(header, block_hash),
        None,
        Some(U256::from(block_length)),
    );

    Block { header, uncles, transactions, withdrawals }
}
