use reth_network::NetworkPrimitives;
use reth_node_api::BlockBody;
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
