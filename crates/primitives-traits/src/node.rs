use crate::{
    Block, FullBlock, FullBlockBody, FullBlockHeader, FullReceipt, FullSignedTx,
    MaybeSerdeBincodeCompat, Receipt,
};
use alloc::sync::Arc;
use core::fmt;

/// Configures all the primitive types of the node.
pub trait NodePrimitives: Send + Sync + Unpin + Sized + fmt::Debug + PartialEq + Eq {
    /// Block primitive.
    type Block: Block<Header = Self::BlockHeader, Body = Self::BlockBody>
        + MaybeSerdeBincodeCompat
        + 'static;
    /// Block header primitive.
    type BlockHeader: FullBlockHeader + 'static;
    /// Block body primitive.
    type BlockBody: FullBlockBody<Transaction = Self::SignedTx, OmmerHeader = Self::BlockHeader>
        + 'static;
    /// Signed version of the transaction type.
    type SignedTx: FullSignedTx + 'static;
    /// A receipt.
    type Receipt: Receipt + 'static;
}

impl<T: NodePrimitives> NodePrimitives for &T {
    type Block = T::Block;
    type BlockHeader = T::BlockHeader;
    type BlockBody = T::BlockBody;
    type SignedTx = T::SignedTx;
    type Receipt = T::Receipt;
}

impl<T: NodePrimitives> NodePrimitives for Arc<T> {
    type Block = T::Block;
    type BlockHeader = T::BlockHeader;
    type BlockBody = T::BlockBody;
    type SignedTx = T::SignedTx;
    type Receipt = T::Receipt;
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
        > + Default
        + Clone
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
        > + Default
        + Clone
        + 'static
{
}

/// Helper adapter type for accessing [`NodePrimitives`] block header types.
pub type HeaderTy<N> = <N as NodePrimitives>::BlockHeader;

/// Helper adapter type for accessing [`NodePrimitives`] block body types.
pub type BodyTy<N> = <N as NodePrimitives>::BlockBody;

/// Helper adapter type for accessing [`NodePrimitives`] block types.
pub type BlockTy<N> = <N as NodePrimitives>::Block;

/// Helper adapter type for accessing [`NodePrimitives`] receipt types.
pub type ReceiptTy<N> = <N as NodePrimitives>::Receipt;

/// Helper adapter type for accessing [`NodePrimitives`] signed transaction types.
pub type TxTy<N> = <N as NodePrimitives>::SignedTx;
