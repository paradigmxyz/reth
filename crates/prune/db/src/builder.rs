use crate::set::from_components;
use alloy_eips::Encodable2718;
use reth_db_api::{table::Value, transaction::DbTxMut};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::StaticFileProvider, BlockReader, ChainStateBlockReader, DBProvider,
    DatabaseProviderFactory, NodePrimitivesProvider, PruneCheckpointReader, PruneCheckpointWriter,
    StaticFileProviderFactory,
};
use reth_prune::{segments::SegmentSet, Pruner, PrunerBuilder};

/// Builds a [Pruner] from the current configuration with the given provider factory.
pub fn build_with_provider_factory<PF>(
    builder: PrunerBuilder,
    provider_factory: PF,
) -> Pruner<PF::ProviderRW, PF>
where
    PF: DatabaseProviderFactory<
            ProviderRW: PruneCheckpointWriter
                            + PruneCheckpointReader
                            + BlockReader<Transaction: Encodable2718>
                            + ChainStateBlockReader
                            + StaticFileProviderFactory<
                Primitives: NodePrimitives<SignedTx: Value, Receipt: Value, BlockHeader: Value>,
            >,
        > + StaticFileProviderFactory<
            Primitives = <PF::ProviderRW as NodePrimitivesProvider>::Primitives,
        >,
{
    let segments: SegmentSet<PF::ProviderRW> =
        from_components(provider_factory.static_file_provider(), builder.segments);

    Pruner::new_with_factory(
        provider_factory,
        segments.into_vec(),
        builder.block_interval,
        builder.delete_limit,
        builder.timeout,
        builder.finished_exex_height,
    )
}

/// Builds a [Pruner] from the current configuration with the given static file provider.
pub fn build<Provider>(
    builder: PrunerBuilder,
    static_file_provider: StaticFileProvider<Provider::Primitives>,
) -> Pruner<Provider, ()>
where
    Provider: StaticFileProviderFactory<
            Primitives: NodePrimitives<SignedTx: Value, Receipt: Value, BlockHeader: Value>,
        > + DBProvider<Tx: DbTxMut>
        + BlockReader<Transaction: Encodable2718>
        + ChainStateBlockReader
        + PruneCheckpointWriter
        + PruneCheckpointReader,
{
    let segments: SegmentSet<Provider> = from_components(static_file_provider, builder.segments);

    Pruner::new(
        segments.into_vec(),
        builder.block_interval,
        builder.delete_limit,
        builder.timeout,
        builder.finished_exex_height,
    )
}
