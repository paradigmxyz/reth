//! Provides a local dev service engine that can be used to run a dev chain.
//!
//! [`LocalEngineService`] polls the payload builder based on a mining mode
//! which can be set to `Instant` or `Interval`. The `Instant` mode will
//! constantly poll the payload builder and initiate block building
//! with a single transaction. The `Interval` mode will initiate block
//! building at a fixed interval.

use crate::miner::MiningMode;
use eyre::eyre;
use reth_beacon_consensus::EngineNodeTypes;
use reth_chain_state::{CanonicalInMemoryState, ExecutedBlock, NewCanonicalChain};
use reth_engine_tree::persistence::PersistenceHandle;
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{
    BuiltPayload, PayloadAttributesBuilder, PayloadBuilder, PayloadBuilderAttributes, PayloadTypes,
};
use reth_provider::ProviderFactory;
use reth_prune::PrunerWithFactory;
use reth_stages_api::MetricEventsSender;
use tokio::sync::oneshot;
use tracing::debug;

/// Provides a local dev service engine that can be used to drive the
/// chain forward.
#[derive(Debug)]
pub struct LocalEngineService<N, B>
where
    N: EngineNodeTypes,
    B: PayloadAttributesBuilder<PayloadAttributes = <N::Engine as PayloadTypes>::PayloadAttributes>,
{
    /// The payload builder for the engine
    payload_builder: PayloadBuilderHandle<N::Engine>,
    /// The payload attribute builder for the engine
    payload_attributes_builder: B,
    /// Keep track of the Canonical chain state that isn't persisted on disk yet
    canonical_in_memory_state: CanonicalInMemoryState,
    /// A handle to the persistence layer
    persistence_handle: PersistenceHandle,
    /// The mining mode for the engine
    mode: MiningMode,
}

impl<N, B> LocalEngineService<N, B>
where
    N: EngineNodeTypes,
    B: PayloadAttributesBuilder<PayloadAttributes = <N::Engine as PayloadTypes>::PayloadAttributes>,
{
    /// Constructor for [`LocalEngineService`].
    pub fn new(
        payload_builder: PayloadBuilderHandle<N::Engine>,
        payload_attributes_builder: B,
        provider: ProviderFactory<N>,
        pruner: PrunerWithFactory<ProviderFactory<N>>,
        canonical_in_memory_state: CanonicalInMemoryState,
        sync_metrics_tx: MetricEventsSender,
        mode: MiningMode,
    ) -> Self {
        let persistence_handle =
            PersistenceHandle::spawn_service(provider, pruner, sync_metrics_tx);

        Self {
            payload_builder,
            payload_attributes_builder,
            canonical_in_memory_state,
            persistence_handle,
            mode,
        }
    }

    /// Spawn the [`LocalEngineService`] on a tokio green thread. The service will poll the payload
    /// builder with two varying modes, [`MiningMode::Instant`] or [`MiningMode::Interval`]
    /// which will respectively either execute the block as soon as it finds a
    /// transaction in the pool or build the block based on an interval.
    pub fn spawn_new(
        payload_builder: PayloadBuilderHandle<N::Engine>,
        payload_attributes_builder: B,
        provider: ProviderFactory<N>,
        pruner: PrunerWithFactory<ProviderFactory<N>>,
        canonical_in_memory_state: CanonicalInMemoryState,
        sync_metrics_tx: MetricEventsSender,
        mode: MiningMode,
    ) {
        let engine = Self::new(
            payload_builder,
            payload_attributes_builder,
            provider,
            pruner,
            canonical_in_memory_state,
            sync_metrics_tx,
            mode,
        );

        // Spawn the engine
        tokio::spawn(engine.run());
    }

    /// Runs the [`LocalEngineService`] in a loop, polling the miner and building
    /// payloads.
    async fn run(mut self) {
        loop {
            // Wait for the interval or the pool to receive a transaction
            (&mut self.mode).await;

            // Start a new payload building job
            let executed_block = self.build_and_save_payload().await;

            if executed_block.is_err() {
                debug!(target: "local_engine", err = ?executed_block.unwrap_err(), "failed payload building");
                continue
            }
            let block = executed_block.expect("not error");

            let res = self.update_canonical_in_memory_state(block);
            if res.is_err() {
                debug!(target: "local_engine", err = ?res.unwrap_err(), "failed canonical state update");
            }
        }
    }

    /// Builds a payload by initiating a new payload job via the [`PayloadBuilderHandle`],
    /// saving the execution outcome to persistence and returning the executed block.
    async fn build_and_save_payload(&self) -> eyre::Result<ExecutedBlock> {
        let payload_attributes = self.payload_attributes_builder.build()?;
        let parent = self.canonical_in_memory_state.get_canonical_head().hash();
        let payload_builder_attributes =
            <N::Engine as PayloadTypes>::PayloadBuilderAttributes::try_new(
                parent,
                payload_attributes,
            )
            .map_err(|_| eyre::eyre!("failed to fetch payload attributes"))?;

        let payload = self
            .payload_builder
            .send_and_resolve_payload(payload_builder_attributes)
            .await?
            .await?;

        let executed_block =
            payload.executed_block().ok_or_else(|| eyre!("missing executed block"))?;
        let (tx, rx) = oneshot::channel();

        let _ = self.persistence_handle.save_blocks(vec![executed_block.clone()], tx);

        // Wait for the persistence_handle to complete
        let _ = rx.await?.ok_or_else(|| eyre!("missing new head"))?;

        Ok(executed_block)
    }

    /// Update the canonical in memory state and send notification for a new canon state to
    /// all the listeners.
    fn update_canonical_in_memory_state(&self, executed_block: ExecutedBlock) -> eyre::Result<()> {
        let chain = NewCanonicalChain::Commit { new: vec![executed_block] };
        let tip = chain.tip().header.clone();
        let notification = chain.to_chain_notification();

        // Update the tracked in-memory state with the new chain
        self.canonical_in_memory_state.update_chain(chain);
        self.canonical_in_memory_state.set_canonical_head(tip);

        // Sends an event to all active listeners about the new canonical chain
        self.canonical_in_memory_state.notify_canon_state(notification);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::MAINNET;
    use reth_config::PruneConfig;
    use reth_db::test_utils::{create_test_rw_db, create_test_static_files_dir};
    use reth_ethereum_engine_primitives::EthEngineTypes;
    use reth_exex_test_utils::TestNode;
    use reth_node_types::NodeTypesWithDBAdapter;
    use reth_payload_builder::test_utils::spawn_test_payload_service;
    use reth_provider::{providers::StaticFileProvider, BlockReader, ProviderFactory};
    use reth_prune::PrunerBuilder;
    use reth_transaction_pool::{
        test_utils::{testing_pool, MockTransaction},
        TransactionPool,
    };
    use std::{convert::Infallible, time::Duration};
    use tokio::sync::mpsc::unbounded_channel;

    #[derive(Debug)]
    struct TestPayloadAttributesBuilder;

    impl PayloadAttributesBuilder for TestPayloadAttributesBuilder {
        type PayloadAttributes = alloy_rpc_types_engine::PayloadAttributes;
        type Error = Infallible;

        fn build(&self) -> Result<Self::PayloadAttributes, Self::Error> {
            Ok(alloy_rpc_types_engine::PayloadAttributes {
                timestamp: 0,
                prev_randao: Default::default(),
                suggested_fee_recipient: Default::default(),
                withdrawals: None,
                parent_beacon_block_root: None,
            })
        }
    }

    #[tokio::test]
    async fn test_local_engine_service_interval() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        // Start the provider and the pruner
        let (_, static_dir_path) = create_test_static_files_dir();
        let provider = ProviderFactory::<NodeTypesWithDBAdapter<TestNode, _>>::new(
            create_test_rw_db(),
            MAINNET.clone(),
            StaticFileProvider::read_write(static_dir_path)?,
        );
        let pruner = PrunerBuilder::new(PruneConfig::default())
            .build_with_provider_factory(provider.clone());

        // Create an empty canonical in memory state
        let canonical_in_memory_state = CanonicalInMemoryState::empty();

        // Start the payload builder service
        let payload_handle = spawn_test_payload_service::<EthEngineTypes>();

        // Sync metric channel
        let (sync_metrics_tx, _) = unbounded_channel();

        // Launch the LocalEngineService in interval mode
        let period = Duration::from_secs(1);
        LocalEngineService::spawn_new(
            payload_handle,
            TestPayloadAttributesBuilder,
            provider.clone(),
            pruner,
            canonical_in_memory_state,
            sync_metrics_tx,
            MiningMode::interval(period),
        );

        // Check that we have no block for now
        let block = provider.block_by_number(0)?;
        assert!(block.is_none());

        // Wait 4 intervals
        tokio::time::sleep(2 * period).await;

        // Assert a block has been build
        let block = provider.block_by_number(0)?;
        assert!(block.is_some());

        Ok(())
    }

    #[tokio::test]
    async fn test_local_engine_service_instant() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        // Start the provider and the pruner
        let (_, static_dir_path) = create_test_static_files_dir();
        let provider = ProviderFactory::<NodeTypesWithDBAdapter<TestNode, _>>::new(
            create_test_rw_db(),
            MAINNET.clone(),
            StaticFileProvider::read_write(static_dir_path)?,
        );
        let pruner = PrunerBuilder::new(PruneConfig::default())
            .build_with_provider_factory(provider.clone());

        // Create an empty canonical in memory state
        let canonical_in_memory_state = CanonicalInMemoryState::empty();

        // Start the payload builder service
        let payload_handle = spawn_test_payload_service::<EthEngineTypes>();

        // Start a transaction pool
        let pool = testing_pool();

        // Sync metric channel
        let (sync_metrics_tx, _) = unbounded_channel();

        // Launch the LocalEngineService in instant mode
        LocalEngineService::spawn_new(
            payload_handle,
            TestPayloadAttributesBuilder,
            provider.clone(),
            pruner,
            canonical_in_memory_state,
            sync_metrics_tx,
            MiningMode::instant(pool.clone()),
        );

        // Wait for a small period to assert block building is
        // triggered by adding a transaction to the pool
        let period = Duration::from_millis(500);
        tokio::time::sleep(period).await;
        let block = provider.block_by_number(0)?;
        assert!(block.is_none());

        // Add a transaction to the pool
        let transaction = MockTransaction::legacy().with_gas_price(10);
        pool.add_transaction(Default::default(), transaction).await?;

        // Wait for block building
        let period = Duration::from_secs(2);
        tokio::time::sleep(period).await;

        // Assert a block has been build
        let block = provider.block_by_number(0)?;
        assert!(block.is_some());

        Ok(())
    }

    #[tokio::test]
    async fn test_canonical_chain_subscription() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        // Start the provider and the pruner
        let (_, static_dir_path) = create_test_static_files_dir();
        let provider = ProviderFactory::<NodeTypesWithDBAdapter<TestNode, _>>::new(
            create_test_rw_db(),
            MAINNET.clone(),
            StaticFileProvider::read_write(static_dir_path)?,
        );
        let pruner = PrunerBuilder::new(PruneConfig::default())
            .build_with_provider_factory(provider.clone());

        // Create an empty canonical in memory state
        let canonical_in_memory_state = CanonicalInMemoryState::empty();
        let mut notifications = canonical_in_memory_state.subscribe_canon_state();

        // Start the payload builder service
        let payload_handle = spawn_test_payload_service::<EthEngineTypes>();

        // Start a transaction pool
        let pool = testing_pool();

        // Sync metric channel
        let (sync_metrics_tx, _) = unbounded_channel();

        // Launch the LocalEngineService in instant mode
        LocalEngineService::spawn_new(
            payload_handle,
            TestPayloadAttributesBuilder,
            provider.clone(),
            pruner,
            canonical_in_memory_state,
            sync_metrics_tx,
            MiningMode::instant(pool.clone()),
        );

        // Add a transaction to the pool
        let transaction = MockTransaction::legacy().with_gas_price(10);
        pool.add_transaction(Default::default(), transaction).await?;

        // Check a notification is received for block 0
        let res = notifications.recv().await?;

        assert_eq!(res.tip().number, 0);

        Ok(())
    }
}
