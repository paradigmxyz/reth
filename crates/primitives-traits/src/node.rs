use core::fmt;

use crate::{
    FullBlock, FullBlockBody, FullBlockHeader, FullReceipt, FullSignedTx, FullTxType, MaybeSerde,
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
    /// Signed version of the transaction type, as found in a block.
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
    Block: FullBlock<Header = Self::BlockHeader, Body = Self::BlockBody>,
    BlockHeader: FullBlockHeader,
    BlockBody: FullBlockBody<Transaction = Self::SignedTx>,
    SignedTx: FullSignedTx,
    TxType: FullTxType,
    Receipt: FullReceipt,
>
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
    >
{
}

/// Helper adapter type for accessing [`NodePrimitives`] receipt type.
pub type ReceiptTy<N> = <N as NodePrimitives>::Receipt;

/// Helper adapter type for accessing [`NodePrimitives`] body type.
pub type BodyTy<N> = <N as NodePrimitives>::BlockBody;
