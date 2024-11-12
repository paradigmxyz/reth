use core::fmt;

use crate::{BlockBody, FullBlock, FullReceipt, FullSignedTx};

/// Configures all the primitive types of the node.
pub trait NodePrimitives: Send + Sync + Unpin + Clone + Default + fmt::Debug {
    /// Block primitive.
    type Block: Send + Sync + Unpin + Clone + Default + fmt::Debug + 'static;
    /// Signed version of the transaction type.
    type SignedTx: Send + Sync + Unpin + Clone + Default + fmt::Debug + 'static;
    /// A receipt.
    type Receipt: Send + Sync + Unpin + Clone + Default + fmt::Debug + 'static;
}

impl NodePrimitives for () {
    type Block = ();
    type SignedTx = ();
    type Receipt = ();
}

/// Helper trait that sets trait bounds on [`NodePrimitives`].
pub trait FullNodePrimitives: Send + Sync + Unpin + Clone + Default + fmt::Debug {
    /// Block primitive.
    type Block: FullBlock<Body: BlockBody<SignedTransaction = Self::SignedTx>>;
    /// Signed version of the transaction type.
    type SignedTx: FullSignedTx;
    /// A receipt.
    type Receipt: FullReceipt;
}

impl<T> NodePrimitives for T
where
    T: FullNodePrimitives<Block: 'static, SignedTx: 'static, Receipt: 'static>,
{
    type Block = T::Block;
    type SignedTx = T::SignedTx;
    type Receipt = T::Receipt;
}
