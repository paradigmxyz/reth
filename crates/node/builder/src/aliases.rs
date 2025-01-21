use reth_evm::{ConfigureEvm, ConfigureEvmEnv};
use reth_network::NetworkPrimitives;
use reth_node_api::{BlockBody, NodePrimitives};
use reth_provider::BlockReader;

/// This is a type alias to make type bounds simpler, when we have a [`NetworkPrimitives`] and need
/// a [`BlockReader`] whose associated types match the [`NetworkPrimitives`] associated types.
pub trait BlockReaderFor<N: NetworkPrimitives>:
    BlockReader<
    Block = N::Block,
    Header = N::BlockHeader,
    Transaction = <N::BlockBody as BlockBody>::Transaction,
    Receipt = N::Receipt,
>
{
}

impl<N, T> BlockReaderFor<N> for T
where
    N: NetworkPrimitives,
    T: BlockReader<
        Block = N::Block,
        Header = N::BlockHeader,
        Transaction = <N::BlockBody as BlockBody>::Transaction,
        Receipt = N::Receipt,
    >,
{
}

/// This is a type alias to make type bounds simpler when we have a [`NodePrimitives`] and need a
/// [`ConfigureEvmEnv`] whose associated types match the [`NodePrimitives`] associated types.
pub trait ConfigureEvmEnvFor<N: NodePrimitives>:
    ConfigureEvmEnv<Header = N::BlockHeader, Transaction = N::SignedTx>
{
}

impl<N, C> ConfigureEvmEnvFor<N> for C
where
    N: NodePrimitives,
    C: ConfigureEvmEnv<Header = N::BlockHeader, Transaction = N::SignedTx>,
{
}

/// This is a type alias to make type bounds simpler when we have a [`NodePrimitives`] and need a
/// [`ConfigureEvm`] whose associated types match the [`NodePrimitives`] associated types.
pub trait ConfigureEvmFor<N: NodePrimitives>:
    ConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>
{
}

impl<N, C> ConfigureEvmFor<N> for C
where
    N: NodePrimitives,
    C: ConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>,
{
}
