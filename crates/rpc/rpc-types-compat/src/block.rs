//! Compatibility functions for rpc `Block` type.

use crate::transaction::TransactionCompat;
use alloy_consensus::{transaction::Recovered, BlockBody, BlockHeader, Sealable};
use alloy_primitives::U256;
use alloy_rpc_types_eth::{
    Block, BlockTransactions, BlockTransactionsKind, Header, TransactionInfo,
};
use reth_primitives_traits::{
    Block as BlockTrait, BlockBody as BlockBodyTrait, NodePrimitives, RecoveredBlock,
    SignedTransaction,
};

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
    T: TransactionCompat,
    B: BlockTrait<Body: BlockBodyTrait<Transaction = <T::Primitives as NodePrimitives>::SignedTx>>,
{
    match kind {
        BlockTransactionsKind::Hashes => Ok(from_block_with_tx_hashes::<T::Transaction, B>(block)),
        BlockTransactionsKind::Full => from_block_full::<T, B>(block, tx_resp_builder),
    }
}

/// Create a new [`Block`] response from a [`RecoveredBlock`], using the
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
    let BlockBody { ommers, withdrawals, .. } = body.into_ethereum_body();

    let transactions = BlockTransactions::Hashes(transactions);
    let uncles = ommers.into_iter().map(|h| h.hash_slow()).collect();
    let header = Header::from_consensus(header.into(), None, Some(U256::from(rlp_length)));

    Block { header, uncles, transactions, withdrawals }
}

/// Create a new [`Block`] response from a [`RecoveredBlock`], using the
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
    T: TransactionCompat,
    B: BlockTrait<Body: BlockBodyTrait<Transaction = <T::Primitives as NodePrimitives>::SignedTx>>,
{
    let block_number = block.header().number();
    let base_fee = block.header().base_fee_per_gas();
    let block_length = block.rlp_length();
    let block_hash = Some(block.hash());

    let (block, senders) = block.split_sealed();
    let (header, body) = block.split_sealed_header_body();
    let BlockBody { transactions, ommers, withdrawals } = body.into_ethereum_body();

    let transactions = transactions
        .into_iter()
        .zip(senders)
        .enumerate()
        .map(|(idx, (tx, sender))| {
            let tx_info = TransactionInfo {
                hash: Some(*tx.tx_hash()),
                block_hash,
                block_number: Some(block_number),
                base_fee,
                index: Some(idx as u64),
            };

            tx_resp_builder.fill(Recovered::new_unchecked(tx, sender), tx_info)
        })
        .collect::<Result<Vec<_>, T::Error>>()?;

    let transactions = BlockTransactions::Full(transactions);
    let uncles = ommers.into_iter().map(|h| h.hash_slow()).collect();
    let header = Header::from_consensus(header.into(), None, Some(U256::from(block_length)));

    let block = Block { header, uncles, transactions, withdrawals };

    Ok(block)
}
