//! Compatibility functions for rpc `Block` type.

use crate::transaction::from_recovered_with_block_context;
use alloy_rlp::Encodable;
use reth_primitives::{Block as PrimitiveBlock, Header as PrimitiveHeader, B256, U256};
use reth_rpc_types::{Block, BlockError, BlockTransactions, BlockTransactionsKind, Header};

/// Converts the given primitive block into a [Block] response with the given
/// [BlockTransactionsKind]
///
/// If a `block_hash` is provided, then this is used, otherwise the block hash is computed.
pub fn from_block(
    block: PrimitiveBlock,
    total_difficulty: U256,
    kind: BlockTransactionsKind,
    block_hash: Option<B256>,
) -> Result<Block, BlockError> {
    match kind {
        BlockTransactionsKind::Hashes => {
            Ok(from_block_with_tx_hashes(block, total_difficulty, block_hash))
        }
        BlockTransactionsKind::Full => from_block_full(block, total_difficulty, block_hash),
    }
}

/// Create a new [Block] response from a [primitive block](reth_primitives::Block), using the
/// total difficulty to populate its field in the rpc response.
///
/// This will populate the `transactions` field with only the hashes of the transactions in the
/// block: [BlockTransactions::Hashes]
pub fn from_block_with_tx_hashes(
    block: PrimitiveBlock,
    total_difficulty: U256,
    block_hash: Option<B256>,
) -> Block {
    let block_hash = block_hash.unwrap_or_else(|| block.header.hash_slow());
    let transactions = block.body.iter().map(|tx| tx.hash()).collect();

    from_block_with_transactions(
        block_hash,
        block,
        total_difficulty,
        BlockTransactions::Hashes(transactions),
    )
}

/// Create a new [Block] response from a [primitive block](reth_primitives::Block), using the
/// total difficulty to populate its field in the rpc response.
///
/// This will populate the `transactions` field with the _full_
/// [Transaction](reth_rpc_types::Transaction) objects: [BlockTransactions::Full]
pub fn from_block_full(
    block: PrimitiveBlock,
    total_difficulty: U256,
    block_hash: Option<B256>,
) -> Result<Block, BlockError> {
    let block_hash = block_hash.unwrap_or_else(|| block.header.hash_slow());
    let block_number = block.number;
    let mut transactions = Vec::with_capacity(block.body.len());
    for (idx, tx) in block.body.iter().enumerate() {
        let signed_tx = tx.clone().into_ecrecovered().ok_or(BlockError::InvalidSignature)?;
        transactions.push(from_recovered_with_block_context(
            signed_tx,
            block_hash,
            block_number,
            block.base_fee_per_gas,
            U256::from(idx),
        ))
    }

    Ok(from_block_with_transactions(
        block_hash,
        block,
        total_difficulty,
        BlockTransactions::Full(transactions),
    ))
}

fn from_block_with_transactions(
    block_hash: B256,
    block: PrimitiveBlock,
    total_difficulty: U256,
    transactions: BlockTransactions,
) -> Block {
    let block_length = block.length();
    let uncles = block.ommers.into_iter().map(|h| h.hash_slow()).collect();
    let header = Header::from_primitive_with_hash(block.header.seal(block_hash));
    let withdrawals = if header.withdrawals_root.is_some() { block.withdrawals } else { None };
    Block {
        header,
        uncles,
        transactions,
        total_difficulty: Some(total_difficulty),
        size: Some(U256::from(block_length)),
        withdrawals,
    }
}

/// Build an RPC block response representing
/// an Uncle from its header.
pub fn uncle_block_from_header(header: PrimitiveHeader) -> Block {
    let hash = header.hash_slow();
    let rpc_header = Header::from_primitive_with_hash(header.clone().seal(hash));
    let uncle_block = PrimitiveBlock { header, ..Default::default() };
    let size = Some(U256::from(uncle_block.length()));
    Block {
        uncles: vec![],
        header: rpc_header,
        transactions: BlockTransactions::Uncle,
        withdrawals: Some(vec![]),
        size,
        total_difficulty: None,
    }
}
