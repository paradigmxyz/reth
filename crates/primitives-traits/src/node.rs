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
    /// Signed version of the transaction type.
    type SignedTx: Send + Sync + Unpin + Clone + fmt::Debug + PartialEq + Eq + MaybeSerde + 'static;
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

/// Helper adapter type for accessing [`NodePrimitives`] receipt type.
pub type ReceiptTy<N> = <N as NodePrimitives>::Receipt;
