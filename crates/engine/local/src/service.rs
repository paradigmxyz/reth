//! Provides a local dev service engine that can be used to run a dev chain.
//!
//! [`LocalEngineService`] polls the payload builder based on a mining mode
//! which can be set to `Instant` or `Interval`. The `Instant` mode will
//! constantly poll the payload builder and initiate block building
//! with a single transaction. The `Interval` mode will initiate block
//! building at a fixed interval.

use crate::miner::MiningMode;
use alloy_primitives::B256;
use reth_beacon_consensus::EngineNodeTypes;
use reth_engine_tree::persistence::PersistenceHandle;
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{
    BuiltPayload, PayloadAttributesBuilder, PayloadBuilder, PayloadBuilderAttributes, PayloadTypes,
};
use reth_provider::ProviderFactory;
use reth_prune::PrunerWithFactory;
use reth_stages_api::MetricEventsSender;
use std::fmt::Formatter;
use tokio::sync::oneshot;
use tracing::debug;

/// Provides a local dev service engine that can be used to drive the
/// chain forward.
pub struct LocalEngineService<N, B>
where
    N: EngineNodeTypes,
    B: PayloadAttributesBuilder<PayloadAttributes = <N::Engine as PayloadTypes>::PayloadAttributes>,
{
    /// The payload builder for the engine
    payload_builder: PayloadBuilderHandle<N::Engine>,
    /// The payload attribute builder for the engine
    payload_attributes_builder: B,
    /// A handle to the persistence layer
    persistence_handle: PersistenceHandle,
    /// The hash of the current head
    head: B256,
    /// The mining mode for the engine
    mode: MiningMode,
}

impl<N, B> std::fmt::Debug for LocalEngineService<N, B>
where
    N: EngineNodeTypes,
    B: PayloadAttributesBuilder<PayloadAttributes = <N::Engine as PayloadTypes>::PayloadAttributes>,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEngineService")
            .field("payload_builder", &self.payload_builder)
            .field("payload_attributes_builder", &self.payload_attributes_builder)
            .field("persistence_handle", &self.persistence_handle)
            .field("head", &self.head)
            .field("mode", &self.mode)
            .finish()
    }
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
        sync_metrics_tx: MetricEventsSender,
        head: B256,
        mode: MiningMode,
    ) -> Self {
        let persistence_handle =
            PersistenceHandle::spawn_service(provider, pruner, sync_metrics_tx);

        Self { payload_builder, payload_attributes_builder, persistence_handle, head, mode }
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
        sync_metrics_tx: MetricEventsSender,
        head: B256,
        mode: MiningMode,
    ) {
        let engine = Self::new(
            payload_builder,
            payload_attributes_builder,
            provider,
            pruner,
            sync_metrics_tx,
            head,
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
            let new_head = self.build_and_save_payload().await;

            if new_head.is_err() {
                debug!(target: "local_engine", err = ?new_head.unwrap_err(), "failed payload building");
                continue
            }

            // Update the head
            self.head = new_head.expect("not error");
        }
    }

    /// Builds a payload by initiating a new payload job via the [`PayloadBuilderHandle`],
    /// saving the execution outcome to persistence and returning the current head of the
    /// chain.
    async fn build_and_save_payload(&self) -> eyre::Result<B256> {
        let payload_attributes = self.payload_attributes_builder.build()?;
        let payload_builder_attributes =
            <N::Engine as PayloadTypes>::PayloadBuilderAttributes::try_new(
                self.head,
                payload_attributes,
            )
            .map_err(|_| eyre::eyre!("failed to fetch payload attributes"))?;

        let payload = self
            .payload_builder
            .send_and_resolve_payload(payload_builder_attributes)
            .await?
            .await?;

        let block = payload.executed_block().map(|block| vec![block]).unwrap_or_default();
        let (tx, rx) = oneshot::channel();

        let _ = self.persistence_handle.save_blocks(block, tx);

        // Wait for the persistence_handle to complete
        let new_head = rx.await?.ok_or_else(|| eyre::eyre!("missing new head"))?;

        Ok(new_head.hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
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
            StaticFileProvider::read_write(static_dir_path).unwrap(),
        );
        let pruner = PrunerBuilder::new(PruneConfig::default())
            .build_with_provider_factory(provider.clone());

        // Start the payload builder service
        let payload_handle = spawn_test_payload_service::<EthEngineTypes>();

        // Sync metric channel
        let (sync_metrics_tx, _) = unbounded_channel();

        // Get the attributes for start of block building
        let genesis_hash = B256::random();

        // Launch the LocalEngineService in interval mode
        let period = Duration::from_secs(1);
        LocalEngineService::spawn_new(
            payload_handle,
            TestPayloadAttributesBuilder,
            provider.clone(),
            pruner,
            sync_metrics_tx,
            genesis_hash,
            MiningMode::interval(period),
        );

        // Wait 4 intervals
        tokio::time::sleep(4 * period).await;

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
            StaticFileProvider::read_write(static_dir_path).unwrap(),
        );
        let pruner = PrunerBuilder::new(PruneConfig::default())
            .build_with_provider_factory(provider.clone());

        // Start the payload builder service
        let payload_handle = spawn_test_payload_service::<EthEngineTypes>();

        // Start a transaction pool
        let pool = testing_pool();

        // Sync metric channel
        let (sync_metrics_tx, _) = unbounded_channel();

        // Get the attributes for start of block building
        let genesis_hash = B256::random();

        // Launch the LocalEngineService in instant mode
        LocalEngineService::spawn_new(
            payload_handle,
            TestPayloadAttributesBuilder,
            provider.clone(),
            pruner,
            sync_metrics_tx,
            genesis_hash,
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
}
