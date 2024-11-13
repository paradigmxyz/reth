use core::fmt;

use crate::{FullBlock, FullBlockBody, FullBlockHeader, FullReceipt, FullSignedTx, FullTxType};

/// Configures all the primitive types of the node.
pub trait NodePrimitives: Send + Sync + Unpin + Clone + Default + fmt::Debug {
    /// Block primitive.
    type Block: Send + Sync + Unpin + Clone + Default + fmt::Debug + 'static;
    /// Block header primitive.
    type BlockHeader: Send + Sync + Unpin + Clone + Default + fmt::Debug + 'static;
    /// Block body primitive.
    type BlockBody: Send + Sync + Unpin + Clone + Default + fmt::Debug + 'static;
    /// Signed version of the transaction type.
    type SignedTx: Send + Sync + Unpin + Clone + Default + fmt::Debug + 'static;
    /// Transaction envelope type ID.
    type TxType: Send + Sync + Unpin + Clone + Default + fmt::Debug + 'static;
    /// A receipt.
    type Receipt: Send + Sync + Unpin + Clone + Default + fmt::Debug + 'static;
}

impl NodePrimitives for () {
    type Block = ();
    type BlockHeader = ();
    type BlockBody = ();
    type SignedTx = ();
    type TxType = ();
    type Receipt = ();
}

/// Helper trait that sets trait bounds on [`NodePrimitives`].
pub trait FullNodePrimitives: Send + Sync + Unpin + Clone + Default + fmt::Debug {
    /// Block primitive.
    type Block: FullBlock<Header = Self::BlockHeader, Body = Self::BlockBody>;
    /// Block header primitive.
    type BlockHeader: FullBlockHeader;
    /// Block body primitive.
    type BlockBody: FullBlockBody<SignedTransaction = Self::SignedTx>;
    /// Signed version of the transaction type.
    type SignedTx: FullSignedTx;
    /// Transaction envelope type ID.
    type TxType: FullTxType;
    /// A receipt.
    type Receipt: FullReceipt;
}

impl<T> NodePrimitives for T
where
    T: FullNodePrimitives<
        Block: 'static,
        BlockHeader: 'static,
        BlockBody: 'static,
        SignedTx: 'static,
        Receipt: 'static,
        TxType: 'static,
    >,
{
    type Block = T::Block;
    type BlockHeader = T::BlockHeader;
    type BlockBody = T::BlockBody;
    type SignedTx = T::SignedTx;
    type TxType = T::TxType;
    type Receipt = T::Receipt;
}
