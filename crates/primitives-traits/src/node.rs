use crate::{
    Block, FullBlock, FullBlockBody, FullBlockHeader, FullReceipt, FullSignedTx,
    MaybeSerdeBincodeCompat, Receipt,
};
use core::fmt;

/// Configures all the primitive types of the node.
pub trait NodePrimitives:
    Send + Sync + Unpin + Clone + Default + fmt::Debug + PartialEq + Eq + 'static
{
    /// Block primitive.
    type Block: Block<Header = Self::BlockHeader, Body = Self::BlockBody> + MaybeSerdeBincodeCompat;
    /// Block header primitive.
    type BlockHeader: FullBlockHeader;
    /// Block body primitive.
    type BlockBody: FullBlockBody<Transaction = Self::SignedTx, OmmerHeader = Self::BlockHeader>;
    /// Signed version of the transaction type.
    type SignedTx: FullSignedTx;
    /// A receipt.
    type Receipt: Receipt;
}
/// Helper trait that sets trait bounds on [`NodePrimitives`].
pub trait FullNodePrimitives
where
    Self: NodePrimitives<
            Block: FullBlock<Header = Self::BlockHeader, Body = Self::BlockBody>,
            BlockHeader: FullBlockHeader,
            BlockBody: FullBlockBody<Transaction = Self::SignedTx>,
            SignedTx: FullSignedTx,
            Receipt: FullReceipt,
        > + Send
        + Sync
        + Unpin
        + Clone
        + Default
        + fmt::Debug
        + PartialEq
        + Eq
        + 'static,
{
}

impl<T> FullNodePrimitives for T where
    T: NodePrimitives<
            Block: FullBlock<Header = Self::BlockHeader, Body = Self::BlockBody>,
            BlockHeader: FullBlockHeader,
            BlockBody: FullBlockBody<Transaction = Self::SignedTx>,
            SignedTx: FullSignedTx,
            Receipt: FullReceipt,
        > + Send
        + Sync
        + Unpin
        + Clone
        + Default
        + fmt::Debug
        + PartialEq
        + Eq
        + 'static
{
}

/// Helper adapter type for accessing [`NodePrimitives`] block header types.
pub type HeaderTy<N> = <N as NodePrimitives>::BlockHeader;

/// Helper adapter type for accessing [`NodePrimitives`] block body types.
pub type BodyTy<N> = <N as NodePrimitives>::BlockBody;

/// Helper adapter type for accessing [`NodePrimitives`] receipt types.
pub type ReceiptTy<N> = <N as NodePrimitives>::Receipt;
