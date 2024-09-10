//! Provides a local dev service engine that can be used to run a dev chain.
//!
//! [`LocalEngineService`] polls the payload builder based on a mining mode
//! which can be set to `Instant` or `Interval`. The `Instant` mode will
//! constantly poll the payload builder and initiate block building
//! with a single transaction. The `Interval` mode will initiate block
//! building at a fixed interval.

use crate::miner::MiningMode;
use futures_util::{future::BoxFuture, FutureExt};
use reth_beacon_consensus::EngineNodeTypes;
use reth_engine_tree::persistence::PersistenceHandle;
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{BuiltPayload, PayloadBuilderAttributes, PayloadTypes};
use reth_primitives::B256;
use reth_provider::ProviderFactory;
use reth_prune::Pruner;
use std::{
    fmt::Formatter,
    future::Future,
    pin::{pin, Pin},
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;
use tracing::debug;

/// Provides a local dev service engine that can be used to drive the
/// chain forward.
pub struct LocalEngineService<N>
where
    N: EngineNodeTypes,
{
    /// The payload builder for the engine
    payload_builder: PayloadBuilderHandle<N::Engine>,
    /// The attributes for the payload builder
    payload_attributes: <N::Engine as PayloadTypes>::PayloadAttributes,
    /// A handle to the persistence layer
    persistence_handle: PersistenceHandle,
    /// The hash of the current head
    head: B256,
    /// The mining mode for the engine
    mode: MiningMode,
    /// Payload request task
    payload_request_state: RequestState,
}

impl<N: EngineNodeTypes> std::fmt::Debug for LocalEngineService<N> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEngineService")
            .field("payload_builder", &self.payload_builder)
            .field("payload_attributes", &self.payload_attributes)
            .field("persistence_handle", &self.persistence_handle)
            .field("head", &self.head)
            .field("mode", &self.mode)
            .field("payload_request_state", &"...")
            .finish()
    }
}

/// The internal state of the [`LocalEngineService`]'s polling task
#[derive(Default)]
struct RequestState {
    ready: bool,
    task: Option<BoxFuture<'static, eyre::Result<B256>>>,
}

impl<N> LocalEngineService<N>
where
    N: EngineNodeTypes,
{
    /// Constructor for [`LocalEngineService`].
    pub fn new(
        payload_builder: PayloadBuilderHandle<N::Engine>,
        payload_attributes: <N::Engine as PayloadTypes>::PayloadAttributes,
        provider: ProviderFactory<N>,
        pruner: Pruner<N::DB, ProviderFactory<N>>,
        head: B256,
        mode: MiningMode,
    ) -> Self {
        let persistence_handle = PersistenceHandle::spawn_service(provider, pruner);

        Self {
            payload_builder,
            payload_attributes,
            persistence_handle,
            head,
            mode,
            payload_request_state: RequestState::default(),
        }
    }

    /// Spawn a [`LocalEngineService`] on a tokio green thread.
    pub fn spawn_new(
        payload_builder: PayloadBuilderHandle<N::Engine>,
        payload_attributes: <N::Engine as PayloadTypes>::PayloadAttributes,
        provider: ProviderFactory<N>,
        pruner: Pruner<N::DB, ProviderFactory<N>>,
        head: B256,
        mode: MiningMode,
    ) {
        let engine = Self::new(payload_builder, payload_attributes, provider, pruner, head, mode);

        // Spawn the engine
        tokio::spawn(engine);
    }
}

/// Run the [`LocalEnginService`] as a Future. The service will poll the payload builder
/// with two varying modes, [`MiningMode::Instant`] or [`MiningMode::Interval`]
/// which will respectively either execute the block as soon as it finds a
/// transaction in the pool or build the block based on an interval.
impl<N> Future for LocalEngineService<N>
where
    N: EngineNodeTypes,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        const MAX_RETRIES: usize = 5;
        let mut retries = 0;
        loop {
            // Wait for the interval or the pool to receive a transaction
            if !this.payload_request_state.ready {
                ready!(pin!(&mut this.mode).poll(cx));
                this.payload_request_state.ready = true;
            }

            if this.payload_request_state.task.is_none() {
                let head = this.head;
                let attributes = this.payload_attributes.clone();
                let payload_builder = this.payload_builder.clone();
                let persistence_handle = this.persistence_handle.clone();

                let task = Box::pin(async move {
                    let attr = <N::Engine as PayloadTypes>::PayloadBuilderAttributes::try_new(
                        head, attributes,
                    )
                    .map_err(|_| eyre::eyre!("failed to fetch payload attributes"))?;

                    let rx = payload_builder.send_new_payload(attr);
                    let id = rx.await??;

                    let payload = payload_builder
                        .best_payload(id)
                        .await
                        .ok_or_else(|| eyre::eyre!("failed to fetch best payload"))??;

                    let block =
                        payload.executed_block().map(|block| vec![block]).unwrap_or_default();
                    let (tx, rx) = oneshot::channel();

                    let _ = persistence_handle.save_blocks(block, tx);

                    // Wait for the persistence_handle to complete
                    let new_head = rx.await?.ok_or_else(|| eyre::eyre!("missing new head"))?;

                    Result::<_, eyre::Error>::Ok(new_head.hash)
                });
                this.payload_request_state.task = Some(task);
            }

            if this.payload_request_state.task.is_some() {
                let mut task = this.payload_request_state.task.take().unwrap();
                match task.poll_unpin(cx) {
                    Poll::Ready(Ok(head)) => {
                        this.head = head;
                        this.payload_request_state.ready = false;
                        continue
                    }
                    Poll::Ready(Err(err)) => {
                        // if we get an error, retry directly by creating a new payload request task
                        debug!(target: "local_engine", ?err, "failed block production");
                        // In order to avoid resource starvation, we break after 5 retries
                        if retries == MAX_RETRIES {
                            this.payload_request_state.ready = false;
                            break
                        }
                        retries += 1;
                        continue
                    }
                    Poll::Pending => {
                        this.payload_request_state.task = Some(task);
                        break
                    }
                }
            }
        }
        Poll::Pending
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
    use reth_primitives::B256;
    use reth_provider::{providers::StaticFileProvider, BlockReader, ProviderFactory};
    use reth_prune::PrunerBuilder;
    use reth_rpc_types::engine::PayloadAttributes;
    use reth_transaction_pool::{
        test_utils::{testing_pool, MockTransaction},
        TransactionPool,
    };
    use std::time::Duration;

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

        // Get the attributes for start of block building
        let attributes = PayloadAttributes {
            timestamp: 0,
            prev_randao: Default::default(),
            suggested_fee_recipient: Default::default(),
            withdrawals: None,
            parent_beacon_block_root: None,
        };
        let genesis_hash = B256::random();

        // Launch the LocalEngineService in interval mode
        let period = Duration::from_secs(1);
        LocalEngineService::spawn_new(
            payload_handle,
            attributes,
            provider.clone(),
            pruner,
            genesis_hash,
            MiningMode::interval(period),
        );

        // Wait 2 interval
        tokio::time::sleep(4 * period).await;

        // Assert a block has been build
        let block = provider.block_by_number(0)?.map(|b| b.seal_slow());
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

        // Get the attributes for start of block building
        let attributes = PayloadAttributes {
            timestamp: 0,
            prev_randao: Default::default(),
            suggested_fee_recipient: Default::default(),
            withdrawals: None,
            parent_beacon_block_root: None,
        };
        let genesis_hash = B256::random();

        // Launch the LocalEngineService in instant mode
        LocalEngineService::spawn_new(
            payload_handle,
            attributes,
            provider.clone(),
            pruner,
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
        let block = provider.block_by_number(0)?.map(|b| b.seal_slow());
        assert!(block.is_some());

        Ok(())
    }
}
