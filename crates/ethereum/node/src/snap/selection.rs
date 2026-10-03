//! Picks the engine's backfill at launch: the staged pipeline, or snap sync with `--snap.v2`.

use super::{SnapBackfillSync, SnapHandoff};
use reth_chainspec::{EthereumHardfork, EthereumHardforks};
use reth_engine_tree::backfill::{BackfillAction, BackfillEvent, BackfillSync, PipelineSync};
use reth_network_p2p::{headers::client::HeadersClient, snap::client::SnapClient};
use reth_node_builder::sync::{BackfillContext, BackfillSyncBuilder};
use reth_provider::{
    providers::ProviderNodeTypes, ChainSpecProvider, DatabaseProviderFactory, ProviderFactory,
};
use reth_tracing::tracing::warn;
use std::{
    task::{Context, Poll},
    time::{SystemTime, UNIX_EPOCH},
};

/// Builds the Ethereum node's backfill: the staged pipeline, or snap sync with `--snap.v2`.
#[derive(Debug, Clone, Copy)]
pub struct EthereumBackfill {
    // Whether snap sync is opted into.
    snap_v2: bool,
}

impl EthereumBackfill {
    /// Creates the builder for the `--snap.v2` setting.
    pub const fn new(snap_v2: bool) -> Self {
        Self { snap_v2 }
    }

    // Snap pivots need block access lists, so a chain where Amsterdam isn't active yet keeps the
    // staged pipeline even with `--snap.v2`.
    fn snap(&self, chain_spec: &impl EthereumHardforks) -> bool {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        self.snap_v2 &&
            chain_spec.is_ethereum_fork_active_at_timestamp(EthereumHardfork::Amsterdam, now)
    }
}

impl<N, C> BackfillSyncBuilder<N, C> for EthereumBackfill
where
    N: ProviderNodeTypes<ChainSpec: EthereumHardforks>,
    C: SnapClient + HeadersClient + Clone + Unpin + 'static,
{
    type Backfill = EthereumBackfillSync<N, C>;

    fn build(self, ctx: BackfillContext<N, C>) -> eyre::Result<Self::Backfill> {
        let snap = self.snap(&*ctx.provider_factory().chain_spec());
        // Genesis has stored the storage settings by now, so the layout check covers a fresh
        // database too.
        ctx.provider_factory().database_provider_ro()?.ensure_sync_mode(snap)?;
        let runtime = ctx.runtime().clone();
        if !snap {
            return Ok(EthereumBackfillSync::Pipeline(PipelineSync::new(
                ctx.into_pipeline(),
                runtime,
            )))
        }
        let client = ctx.client().clone();
        let provider_factory = ctx.provider_factory().clone();
        Ok(EthereumBackfillSync::Snap(Box::new(SnapBackfillSync::new(
            ctx.into_pipeline(),
            client,
            provider_factory,
            runtime,
        ))))
    }

    fn recover(&mut self, provider_factory: &ProviderFactory<N>) -> eyre::Result<()> {
        let snap = self.snap(&*provider_factory.chain_spec());
        if self.snap_v2 && !snap {
            warn!(target: "sync::snap", "Amsterdam isn't active yet, so --snap.v2 keeps the staged pipeline");
        }
        SnapHandoff::new(provider_factory.clone()).resume_interrupted_publish()?;
        provider_factory.database_provider_ro()?.ensure_sync_mode(snap)?;
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
    C: SnapClient + Clone + Unpin + 'static,
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
    use alloy_eips::BlockNumHash;
    use alloy_primitives::B256;
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_network_p2p::NoopFullBlockClient;
    use reth_provider::{
        test_utils::{create_test_provider_factory_with_chain_spec, MockNodeTypesWithDB},
        DBProvider, MetadataWriter, StorageSettings, StorageSettingsCache,
    };
    use reth_prune::PruneModes;
    use reth_snap_sync::{SnapAttemptStore, SnapGeneration};
    use reth_stages::Pipeline;
    use reth_static_file::StaticFileProducer;
    use reth_tasks::Runtime;
    use std::sync::Arc;

    type TestFactory = ProviderFactory<MockNodeTypesWithDB>;

    fn factory(chain_spec: ChainSpec) -> TestFactory {
        create_test_provider_factory_with_chain_spec(Arc::new(chain_spec))
    }

    fn amsterdam() -> ChainSpec {
        ChainSpecBuilder::mainnet().amsterdam_activated().build()
    }

    // The backfill `EthereumBackfill` builds over `factory`, or why it refused to.
    fn try_build(
        snap_v2: bool,
        factory: TestFactory,
    ) -> eyre::Result<EthereumBackfillSync<MockNodeTypesWithDB, NoopFullBlockClient>> {
        let pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
            .with_tip_sender(tokio::sync::watch::channel(B256::ZERO).0)
            .build(
                factory.clone(),
                StaticFileProducer::new(factory.clone(), PruneModes::default()),
            );
        let ctx = BackfillContext::new(
            pipeline,
            NoopFullBlockClient::default(),
            factory,
            Runtime::test(),
        );
        EthereumBackfill::new(snap_v2).build(ctx)
    }

    fn build(
        snap_v2: bool,
        factory: TestFactory,
    ) -> EthereumBackfillSync<MockNodeTypesWithDB, NoopFullBlockClient> {
        try_build(snap_v2, factory).unwrap()
    }

    // Runs `EthereumBackfill`'s startup recovery over `factory`.
    fn recover(snap_v2: bool, factory: &TestFactory) -> eyre::Result<()> {
        BackfillSyncBuilder::<MockNodeTypesWithDB, NoopFullBlockClient>::recover(
            &mut EthereumBackfill::new(snap_v2),
            factory,
        )
    }

    #[test]
    fn the_staged_pipeline_runs_without_the_flag() {
        assert!(matches!(build(false, factory(amsterdam())), EthereumBackfillSync::Pipeline(_)));
    }

    #[test]
    fn the_flag_selects_snap_sync() {
        assert!(matches!(build(true, factory(amsterdam())), EthereumBackfillSync::Snap(_)));
    }

    #[test]
    fn a_chain_without_amsterdam_keeps_the_staged_pipeline() {
        let factory = factory(ChainSpecBuilder::mainnet().build());

        assert!(matches!(build(true, factory.clone()), EthereumBackfillSync::Pipeline(_)));
        assert!(recover(true, &factory).is_ok());
    }

    #[test]
    fn a_chain_before_amsterdam_keeps_the_staged_pipeline() {
        let factory = factory(ChainSpecBuilder::mainnet().with_amsterdam_at(u64::MAX).build());

        assert!(matches!(build(true, factory), EthereumBackfillSync::Pipeline(_)));
    }

    #[test]
    fn building_refuses_snap_into_a_legacy_layout_stored_at_genesis() {
        let factory = factory(amsterdam());
        let provider = factory.database_provider_rw().unwrap();
        provider.write_storage_settings(StorageSettings::v1()).unwrap();
        provider.commit().unwrap();
        // Recovery runs before genesis stores the settings, so only the build can refuse it.
        assert!(try_build(true, factory).is_err());
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

        assert!(recover(false, &factory).is_err());
        assert!(recover(true, &factory).is_ok());
    }
}
