//! Block abstraction.

use alloy_consensus::{BlockHeader, Transaction};

/// Abstraction of block data type.
pub trait Block {
    /// Header part of the block.
    type Header: BlockHeader;
    /// The block's body contains the transactions in the block.
    type Body: BlockBody;
}

/// Abstraction for block's body.
pub trait BlockBody {
    /// Ordered list of signed transactions as committed in block.
    // todo: requires trait for signed transaction
    type SignedTransaction: Transaction;
}

// todo: remove in favour of moving block type to ethereum primitives crate
impl Block for reth_primitives::Block {
    type Header = reth_primitives::Header;
    type Body = reth_primitives::BlockBody;
}

// todo: remove in favour of moving block body type to ethereum primitives crate
impl BlockBody for reth_primitives::BlockBody {
    type SignedTransaction = reth_primitives::TransactionSigned;
}
