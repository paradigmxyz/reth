//! Picks the engine's backfill at launch: the staged pipeline, or snap sync with `--snap.v2`.

use super::{SnapBackfillSync, SnapHandoff};
use reth_chainspec::{EthereumHardfork, EthereumHardforks, ForkCondition};
use reth_engine_tree::backfill::{BackfillAction, BackfillEvent, BackfillSync, PipelineSync};
use reth_network_p2p::{headers::client::HeadersClient, snap::client::SnapClient};
use reth_node_builder::{
    sync::{BackfillContext, BackfillSyncBuilder, PipelineBackfill},
    NodeConfig,
};
use reth_provider::{providers::ProviderNodeTypes, DatabaseProviderFactory, ProviderFactory};
use reth_tracing::tracing::warn;
use std::task::{Context, Poll};

/// Builds the Ethereum node's backfill: the staged pipeline, or snap sync with `--snap.v2`.
#[derive(Debug, Clone, Copy)]
pub struct EthereumBackfill {
    // Whether the backfill snap syncs, decided once at launch.
    snap: bool,
}

impl EthereumBackfill {
    /// Creates the builder for `config`'s `--snap.v2` setting. Snap pivots need block access
    /// lists, so a chain that never schedules Amsterdam keeps the staged pipeline.
    pub fn new<ChainSpec: EthereumHardforks>(config: &NodeConfig<ChainSpec>) -> Self {
        let snap_v2 = config.network.snap_v2;
        let snap = snap_v2 &&
            config.chain.ethereum_fork_activation(EthereumHardfork::Amsterdam) !=
                ForkCondition::Never;
        if snap_v2 && !snap {
            warn!(target: "sync::snap", "This chain has no block access lists, so --snap.v2 keeps the staged pipeline");
        }
        Self { snap }
    }
}

impl<N, C> BackfillSyncBuilder<N, C> for EthereumBackfill
where
    N: ProviderNodeTypes,
    C: SnapClient + HeadersClient + Clone + Unpin + 'static,
{
    type Backfill = EthereumBackfillSync<N, C>;

    fn build(self, ctx: BackfillContext<N, C>) -> eyre::Result<Self::Backfill> {
        // Genesis has stored the storage settings by now, so the layout check covers a fresh
        // database too.
        ctx.provider_factory().database_provider_ro()?.ensure_sync_mode(self.snap)?;
        if !self.snap {
            return PipelineBackfill.build(ctx).map(EthereumBackfillSync::Pipeline)
        }
        let client = ctx.client().clone();
        let provider_factory = ctx.provider_factory().clone();
        let runtime = ctx.runtime().clone();
        Ok(EthereumBackfillSync::Snap(Box::new(SnapBackfillSync::new(
            ctx.into_pipeline(),
            client,
            provider_factory,
            runtime,
        ))))
    }

    fn recover(&mut self, provider_factory: &ProviderFactory<N>) -> eyre::Result<()> {
        SnapHandoff::new(provider_factory.clone()).resume_interrupted_publish()?;
        // The snap layout is checked in `build`, after genesis initializes it.
        if !self.snap {
            provider_factory.database_provider_ro()?.ensure_sync_mode(false)?;
        }
        Ok(())
    }
}

/// The Ethereum node's backfill: the staged pipeline, or snap sync.
#[derive(Debug)]
pub enum EthereumBackfillSync<N: ProviderNodeTypes, C> {
    /// The staged pipeline, which every node runs unless snap sync is opted into.
    Pipeline(PipelineSync<N>),
    /// Snap sync, which downloads state at a pivot before the pipeline continues above it.
    Snap(Box<SnapBackfillSync<N, C>>),
}

impl<N, C> BackfillSync for EthereumBackfillSync<N, C>
where
    N: ProviderNodeTypes,
    C: SnapClient + HeadersClient + Clone + Unpin + 'static,
{
    fn on_action(&mut self, action: BackfillAction) {
        match self {
            Self::Pipeline(sync) => sync.on_action(action),
            Self::Snap(sync) => sync.on_action(action),
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent> {
        match self {
            Self::Pipeline(sync) => sync.poll(cx),
            Self::Snap(sync) => sync.poll(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap::tests::pipeline_on;
    use alloy_eips::BlockNumHash;
    use alloy_primitives::B256;
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_network_p2p::NoopFullBlockClient;
    use reth_node_core::args::NetworkArgs;
    use reth_provider::{
        test_utils::{create_test_provider_factory_with_chain_spec, MockNodeTypesWithDB},
        ChainSpecProvider, DBProvider, MetadataWriter, ProviderError, StageCheckpointWriter,
        StorageSettings, StorageSettingsCache,
    };
    use reth_snap_sync::{SnapAttemptStore, SnapGeneration};
    use reth_stages::{StageCheckpoint, StageId, StageSetBuilder};
    use reth_tasks::Runtime;
    use std::sync::Arc;

    type TestFactory = ProviderFactory<MockNodeTypesWithDB>;

    fn factory(chain_spec: ChainSpec) -> TestFactory {
        create_test_provider_factory_with_chain_spec(Arc::new(chain_spec))
    }

    fn amsterdam() -> ChainSpec {
        ChainSpecBuilder::mainnet().amsterdam_activated().build()
    }

    // The `--snap.v2` setting on `factory`'s chain.
    fn backfill(snap_v2: bool, factory: &TestFactory) -> EthereumBackfill {
        let config = NodeConfig::new(factory.chain_spec())
            .with_network(NetworkArgs { snap_v2, ..Default::default() });
        EthereumBackfill::new(&config)
    }

    // The backfill `EthereumBackfill` builds over `factory`, or why it refused to.
    fn build(
        snap_v2: bool,
        factory: TestFactory,
    ) -> eyre::Result<EthereumBackfillSync<MockNodeTypesWithDB, NoopFullBlockClient>> {
        let pipeline = pipeline_on(
            &factory,
            StageSetBuilder::default(),
            tokio::sync::watch::channel(B256::ZERO).0,
        );
        backfill(snap_v2, &factory).build(BackfillContext::new(
            pipeline,
            NoopFullBlockClient::default(),
            factory,
            Runtime::test(),
        ))
    }

    // Runs `EthereumBackfill`'s startup recovery over `factory`.
    fn recover(snap_v2: bool, factory: &TestFactory) -> eyre::Result<()> {
        BackfillSyncBuilder::<MockNodeTypesWithDB, NoopFullBlockClient>::recover(
            &mut backfill(snap_v2, factory),
            factory,
        )
    }

    #[test]
    fn the_staged_pipeline_runs_without_the_flag() {
        assert!(matches!(
            build(false, factory(amsterdam())).unwrap(),
            EthereumBackfillSync::Pipeline(_)
        ));
    }

    #[test]
    fn the_flag_selects_snap_sync() {
        let factory = factory(amsterdam());
        recover(true, &factory).unwrap();

        // Genesis supplies the layout before the backfill is built.
        factory.set_storage_settings_cache(StorageSettings::v2());
        assert!(matches!(build(true, factory).unwrap(), EthereumBackfillSync::Snap(_)));
    }

    #[test]
    fn a_chain_without_amsterdam_keeps_the_staged_pipeline() {
        let factory = factory(ChainSpecBuilder::mainnet().build());

        recover(true, &factory).unwrap();
        assert!(matches!(build(true, factory).unwrap(), EthereumBackfillSync::Pipeline(_)));
    }

    #[test]
    fn building_refuses_snap_into_a_legacy_layout_stored_at_genesis() {
        let factory = factory(amsterdam());
        // Genesis stores the settings and sets every checkpoint to its block.
        let provider = factory.database_provider_rw().unwrap();
        provider.write_storage_settings(StorageSettings::v1()).unwrap();
        provider.save_stage_checkpoint(StageId::Execution, StageCheckpoint::new(0)).unwrap();
        provider.commit().unwrap();

        // Recovery runs before genesis stores the settings, so only the build can refuse it.
        let error = build(true, factory).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ProviderError>(),
            Some(ProviderError::SnapStorageLayoutUnsupported)
        ));
    }

    #[test]
    fn recovery_refuses_unverified_snap_state_without_the_flag() {
        let factory = factory(amsterdam());
        factory.set_storage_settings_cache(StorageSettings::v2());
        let provider = factory.database_provider_rw().unwrap();
        provider.write_storage_settings(StorageSettings::v2()).unwrap();
        let pivot = BlockNumHash::new(10, B256::repeat_byte(1));
        provider.start_snap_attempt(SnapGeneration::new(pivot, B256::repeat_byte(2))).unwrap();
        provider.commit().unwrap();

        let error = recover(false, &factory).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ProviderError>(),
            Some(ProviderError::SnapStateRequiresSnapSync { .. })
        ));
        recover(true, &factory).unwrap();
    }
}
