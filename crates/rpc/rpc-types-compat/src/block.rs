//! Compatibility functions for rpc `Block` type.

use crate::transaction::TransactionCompat;
use alloy_consensus::{BlockHeader, Sealable};
use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::U256;
use alloy_rpc_types_eth::{
    Block, BlockTransactions, BlockTransactionsKind, Header, TransactionInfo,
};
use reth_primitives::{transaction::SignedTransactionIntoRecoveredExt, RecoveredBlock};
use reth_primitives_traits::{Block as BlockTrait, BlockBody, SealedHeader, SignedTransaction};

/// Converts the given primitive block into a [`Block`] response with the given
/// [`BlockTransactionsKind`]
///
/// If a `block_hash` is provided, then this is used, otherwise the block hash is computed.
#[expect(clippy::type_complexity)]
pub fn from_block<T, B>(
    block: RecoveredBlock<B>,
    kind: BlockTransactionsKind,
    tx_resp_builder: &T,
) -> Result<Block<T::Transaction, Header<B::Header>>, T::Error>
where
    T: TransactionCompat<<<B as BlockTrait>::Body as BlockBody>::Transaction>,
    B: BlockTrait,
{
    match kind {
        BlockTransactionsKind::Hashes => Ok(from_block_with_tx_hashes::<T::Transaction, B>(block)),
        BlockTransactionsKind::Full => from_block_full::<T, B>(block, tx_resp_builder),
    }
}

/// Create a new [`Block`] response from a [primitive block](reth_primitives::Block), using the
/// total difficulty to populate its field in the rpc response.
///
/// This will populate the `transactions` field with only the hashes of the transactions in the
/// block: [`BlockTransactions::Hashes`]
pub fn from_block_with_tx_hashes<T, B>(block: RecoveredBlock<B>) -> Block<T, Header<B::Header>>
where
    B: BlockTrait,
{
    let transactions = block.body().transaction_hashes_iter().copied().collect();
    let rlp_length = block.rlp_length();
    let (header, body) = block.into_sealed_block().split_sealed_header_body();
    from_block_with_transactions::<T, B>(
        rlp_length,
        header,
        body,
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
    block: RecoveredBlock<B>,
    tx_resp_builder: &T,
) -> Result<Block<T::Transaction, Header<B::Header>>, T::Error>
where
    T: TransactionCompat<<<B as BlockTrait>::Body as BlockBody>::Transaction>,
    B: BlockTrait,
{
    let block_number = block.header().number();
    let base_fee_per_gas = block.header().base_fee_per_gas();

    // NOTE: we can safely remove the body here because not needed to finalize the `Block` in
    // `from_block_with_transactions`, however we need to compute the length before
    let block_length = block.rlp_length();
    let transactions = block.body().transactions().to_vec();
    let transactions_with_senders = transactions.into_iter().zip(block.senders_iter().copied());
    let block_hash = Some(block.hash());
    let transactions = transactions_with_senders
        .enumerate()
        .map(|(idx, (tx, sender))| {
            let tx_hash = *tx.tx_hash();
            let signed_tx_ec_recovered = tx.with_signer(sender);
            let tx_info = TransactionInfo {
                hash: Some(tx_hash),
                block_hash,
                block_number: Some(block_number),
                base_fee: base_fee_per_gas.map(u128::from),
                index: Some(idx as u64),
            };

            tx_resp_builder.fill(signed_tx_ec_recovered, tx_info)
        })
        .collect::<Result<Vec<_>, T::Error>>()?;

    let (header, body) = block.into_sealed_block().split_sealed_header_body();
    Ok(from_block_with_transactions::<_, B>(
        block_length,
        header,
        body,
        BlockTransactions::Full(transactions),
    ))
}

#[inline]
fn from_block_with_transactions<T, B: BlockTrait>(
    block_length: usize,
    header: SealedHeader<B::Header>,
    body: B::Body,
    transactions: BlockTransactions<T>,
) -> Block<T, Header<B::Header>> {
    let withdrawals = header
        .withdrawals_root()
        .is_some()
        .then(|| body.withdrawals().cloned().map(Withdrawals::into_inner).map(Into::into))
        .flatten();

    let uncles =
        body.ommers().map(|o| o.iter().map(|h| h.hash_slow()).collect()).unwrap_or_default();
    let header = Header::from_consensus(header.into(), None, Some(U256::from(block_length)));

    Block { header, uncles, transactions, withdrawals }
}
