use alloy_consensus::{BlockBody, Header};
use reth_optimism_primitives::{transaction::signed::OpTransaction, DepositReceipt};
use reth_primitives_traits::{NodePrimitives, SignedTransaction};

/// Helper trait to encapsulate common bounds on [`NodePrimitives`] for OP payload builder.
pub trait OpPayloadPrimitives:
    NodePrimitives<
    Receipt: DepositReceipt,
    SignedTx: SignedTransaction + OpTransaction,
    BlockHeader = Header,
    BlockBody = BlockBody<Self::SignedTx>,
>
{
}

impl<T> OpPayloadPrimitives for T where
    T: NodePrimitives<
        Receipt: DepositReceipt,
        SignedTx: SignedTransaction + OpTransaction,
        BlockHeader = Header,
        BlockBody = BlockBody<Self::SignedTx>,
    >
{
}
