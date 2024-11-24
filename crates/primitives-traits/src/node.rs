use core::fmt;

use crate::{
    Block, BlockBody, BlockHeader, FullBlock, FullBlockBody, FullBlockHeader, FullReceipt,
    FullSignedTx, FullTxType, MaybeSerde, SignedTransaction,
};

/// Configures all the primitive types of the node.
pub trait NodePrimitives:
    Send + Sync + Unpin + Clone + Default + fmt::Debug + PartialEq + Eq + 'static
{
    /// Block primitive.
    type Block: Send
        + Sync
        + Unpin
        + Clone
        + Default
        + fmt::Debug
        + PartialEq
        + Eq
        + MaybeSerde
        + 'static;
    /// Block header primitive.
    type BlockHeader: Send
        + Sync
        + Unpin
        + Clone
        + Default
        + fmt::Debug
        + PartialEq
        + Eq
        + MaybeSerde
        + 'static;
    /// Block body primitive.
    type BlockBody: Send
        + Sync
        + Unpin
        + Clone
        + Default
        + fmt::Debug
        + PartialEq
        + Eq
        + MaybeSerde
        + 'static;
    /// Signed version of the transaction type.
    type SignedTx: Send
        + Sync
        + Unpin
        + Clone
        + Default
        + fmt::Debug
        + PartialEq
        + Eq
        + MaybeSerde
        + 'static;
    /// Transaction envelope type ID.
    type TxType: Send + Sync + Unpin + Clone + Default + fmt::Debug + PartialEq + Eq + 'static;
    /// A receipt.
    type Receipt: Send
        + Sync
        + Unpin
        + Clone
        + Default
        + fmt::Debug
        + PartialEq
        + Eq
        + MaybeSerde
        + 'static;
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
pub trait FullNodePrimitives:
    NodePrimitives<
    Block: FullBlock<Header = Self::BlockHeader, Body = Self::BlockBody> + 'static,
    BlockHeader: FullBlockHeader + 'static,
    BlockBody: FullBlockBody<Transaction = Self::SignedTx> + 'static,
    SignedTx: FullSignedTx + 'static,
    TxType: FullTxType + 'static,
    Receipt: FullReceipt + 'static,
>
{
}

impl<T> FullNodePrimitives for T where
    T: NodePrimitives<
        Block: FullBlock<Header = Self::BlockHeader, Body = Self::BlockBody> + 'static,
        BlockHeader: FullBlockHeader + 'static,
        BlockBody: FullBlockBody<Transaction = Self::SignedTx> + 'static,
        SignedTx: FullSignedTx + 'static,
        TxType: FullTxType + 'static,
        Receipt: FullReceipt + 'static,
    >
{
}

/// Helper adapter type for accessing [`NodePrimitives`] receipt type.
pub type ReceiptTy<N> = <N as NodePrimitives>::Receipt;

/// Helper trait that links [`NodePrimitives::Block`] to [`NodePrimitives::BlockHeader`],
/// [`NodePrimitives::BlockBody`] and [`NodePrimitives::SignedTx`].
pub trait BlockPrimitives:
    NodePrimitives<
    Block: Block<Header = Self::BlockHeader, Body = Self::BlockBody>,
    BlockHeader: BlockHeader,
    BlockBody: BlockBody<Transaction = Self::SignedTx>,
    SignedTx: SignedTransaction,
>
{
}

impl<T> BlockPrimitives for T where
    T: NodePrimitives<
        Block: Block<Header = T::BlockHeader, Body = T::BlockBody>,
        BlockHeader: BlockHeader,
        BlockBody: BlockBody<Transaction = T::SignedTx>,
        SignedTx: SignedTransaction,
    >
{
}

/// Helper trait like [`BlockPrimitives`], but with encoding trait bounds on
/// [`NodePrimitives::Block`], [`NodePrimitives::BlockHeader`], [`NodePrimitives::BlockBody`] and
/// [`NodePrimitives::SignedTx`].
pub trait FullBlockPrimitives:
    NodePrimitives<
    Block: FullBlock<Header = Self::BlockHeader, Body = Self::BlockBody>,
    BlockHeader: FullBlockHeader,
    BlockBody: FullBlockBody<Transaction = Self::SignedTx>,
    SignedTx: FullSignedTx,
>
{
}

impl<T> FullBlockPrimitives for T where
    T: NodePrimitives<
        Block: FullBlock<Header = T::BlockHeader, Body = T::BlockBody>,
        BlockHeader: FullBlockHeader,
        BlockBody: FullBlockBody<Transaction = T::SignedTx>,
        SignedTx: FullSignedTx,
    >
{
}
