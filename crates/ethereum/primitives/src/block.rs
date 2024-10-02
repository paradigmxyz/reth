//! L1 ethereum block primitive.

use derive_more::Deref;
use reth_node_api::{Block, BlockBody};

#[derive(Debug, Deref)]
pub struct EthBlock(reth_primitives::Block);

#[derive(Debug, Deref)]
pub struct EthHeader(reth_primitives::Header);

#[derive(Debug, Deref)]
pub struct EthBlockBody(reth_primitives::BlockBody);

#[derive(Debug, Deref)]
pub struct EthSignedTransaction(reth_primitives::TransactionSigned);

impl Block for EthBlock {
    type Header = EthHeader;
    type Body = EthBlockBody;
}

impl BlockBody for EthBlockBody {
    type SignedTransaction = EthSignedTransaction;
}
