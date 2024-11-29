use core::fmt;

use crate::{
    Block, BlockBody, BlockHeader, FullBlock, FullBlockBody, FullBlockHeader, FullReceipt,
    FullSignedTx, FullTxType, MaybeArbitrary, MaybeSerde, Receipt,
};

/// Configures all the primitive types of the node.
pub trait NodePrimitives:
    Send + Sync + Unpin + Clone + Default + fmt::Debug + PartialEq + Eq + 'static
{
    /// Block primitive.
    type Block: Block<Header = Self::BlockHeader, Body = Self::BlockBody>;
    /// Block header primitive.
    type BlockHeader: BlockHeader;
    /// Block body primitive.
    type BlockBody: BlockBody<Transaction = Self::SignedTx, OmmerHeader = Self::BlockHeader>;
    /// Signed version of the transaction type.
    type SignedTx: Send
        + Sync
        + Unpin
        + Clone
        + fmt::Debug
        + PartialEq
        + Eq
        + MaybeSerde
        + MaybeArbitrary
        + 'static;
    /// Transaction envelope type ID.
    type TxType: Send
        + Sync
        + Unpin
        + Clone
        + Default
        + fmt::Debug
        + PartialEq
        + Eq
        + MaybeArbitrary
        + 'static;
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
            TxType: FullTxType,
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
            TxType: FullTxType,
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
