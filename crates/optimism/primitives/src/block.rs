//! Optimism block primitive.

use derive_more::Deref;
use reth_node_api::{Block, BlockBody};

#[derive(Debug, Deref)]
pub struct OpBlock(reth_primitives::Block);

#[derive(Debug, Deref)]
pub struct OpHeader(reth_primitives::Header);

#[derive(Debug, Deref)]
pub struct OpBlockBody(reth_primitives::BlockBody);

#[derive(Debug, Deref)]
pub struct OpSignedTransaction(reth_primitives::TransactionSigned);

impl Block for OpBlock {
    type Header = OpHeader;
    type Body = OpBlockBody;
}

impl BlockBody for OpBlockBody {
    type SignedTransaction = OpSignedTransaction;
}
