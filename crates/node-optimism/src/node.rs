//! Optimism Node types config.

use crate::{
    args::RollupArgs,
    txpool::{OpTransactionPool, OpTransactionValidator},
    OptimismEngineTypes, OptimismEvmConfig,
};
use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_network::{NetworkHandle, NetworkManager};
use reth_node_builder::{
    components::{ComponentsBuilder, NetworkBuilder, PayloadServiceBuilder, PoolBuilder},
    node::{FullNodeTypes, NodeTypes},
    BuilderContext, Node, PayloadBuilderConfig,
};
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
use reth_provider::CanonStateSubscriptions;
use reth_tracing::tracing::{debug, info};
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, CoinbaseTipOrdering, TransactionPool,
    TransactionValidationTaskExecutor,
};

/// Type configuration for a regular Optimism node.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct OptimismNode {
    /// Additional Optimism args
    pub args: RollupArgs,
}

impl OptimismNode {
    /// Creates a new instance of the Optimism node type.
    pub const fn new(args: RollupArgs) -> Self {
        Self { args }
    }

    /// Returns the components for the given [RollupArgs].
    pub fn components<Node>(
        args: RollupArgs,
    ) -> ComponentsBuilder<Node, OptimismPoolBuilder, OptimismPayloadBuilder, OptimismNetworkBuilder>
    where
        Node: FullNodeTypes<Engine = OptimismEngineTypes>,
    {
        let RollupArgs { sequencer_http, disable_txpool_gossip, compute_pending_block, .. } = args;
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(OptimismPoolBuilder::default())
            .payload(OptimismPayloadBuilder::new(compute_pending_block))
            .network(OptimismNetworkBuilder { sequencer_http, disable_txpool_gossip })
    }
}

impl<N> Node<N> for OptimismNode
where
    N: FullNodeTypes<Engine = OptimismEngineTypes>,
{
    type PoolBuilder = OptimismPoolBuilder;
    type NetworkBuilder = OptimismNetworkBuilder;
    type PayloadBuilder = OptimismPayloadBuilder;

    fn components(
        self,
    ) -> ComponentsBuilder<N, Self::PoolBuilder, Self::PayloadBuilder, Self::NetworkBuilder> {
        let Self { args } = self;
        Self::components(args)
    }
}

impl NodeTypes for OptimismNode {
    type Primitives = ();
    type Engine = OptimismEngineTypes;
    type Evm = OptimismEvmConfig;

    fn evm_config(&self) -> Self::Evm {
        OptimismEvmConfig::default()
    }
}

/// A basic optimism transaction pool.
///
/// This contains various settings that can be configured and take precedence over the node's
/// config.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct OptimismPoolBuilder;

impl<Node> PoolBuilder<Node> for OptimismPoolBuilder
where
    Node: FullNodeTypes,
{
    type Pool = OpTransactionPool<Node::Provider, DiskFileBlobStore>;

    async fn build_pool(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        let data_dir = ctx.data_dir();
        let blob_store = DiskFileBlobStore::open(data_dir.blobstore_path(), Default::default())?;

        let validator = TransactionValidationTaskExecutor::eth_builder(ctx.chain_spec())
            .with_head_timestamp(ctx.head().timestamp)
            .kzg_settings(ctx.kzg_settings()?)
            .with_additional_tasks(1)
            .build_with_tasks(
                ctx.provider().clone(),
                ctx.task_executor().clone(),
                blob_store.clone(),
            )
            .map(OpTransactionValidator::new);

        let transaction_pool = reth_transaction_pool::Pool::new(
            validator,
            CoinbaseTipOrdering::default(),
            blob_store,
            ctx.pool_config(),
        );
        info!(target: "reth::cli", "Transaction pool initialized");
        let transactions_path = data_dir.txpool_transactions_path();

        // spawn txpool maintenance task
        {
            let pool = transaction_pool.clone();
            let chain_events = ctx.provider().canonical_state_stream();
            let client = ctx.provider().clone();
            let transactions_backup_config =
                reth_transaction_pool::maintain::LocalTransactionBackupConfig::with_local_txs_backup(transactions_path);

            ctx.task_executor().spawn_critical_with_graceful_shutdown_signal(
                "local transactions backup task",
                |shutdown| {
                    reth_transaction_pool::maintain::backup_local_transactions_task(
                        shutdown,
                        pool.clone(),
                        transactions_backup_config,
                    )
                },
            );

            // spawn the maintenance task
            ctx.task_executor().spawn_critical(
                "txpool maintenance task",
                reth_transaction_pool::maintain::maintain_transaction_pool_future(
                    client,
                    pool,
                    chain_events,
                    ctx.task_executor().clone(),
                    Default::default(),
                ),
            );
            debug!(target: "reth::cli", "Spawned txpool maintenance task");
        }

        Ok(transaction_pool)
    }
}

/// A basic optimism payload service builder
#[derive(Debug, Default, Clone)]
pub struct OptimismPayloadBuilder {
    /// By default the pending block equals the latest block
    /// to save resources and not leak txs from the tx-pool,
    /// this flag enables computing of the pending block
    /// from the tx-pool instead.
    ///
    /// If `compute_pending_block` is not enabled, the payload builder
    /// will use the payload attributes from the latest block. Note
    /// that this flag is not yet functional.
    pub compute_pending_block: bool,
}

impl OptimismPayloadBuilder {
    /// Create a new instance with the given `compute_pending_block` flag.
    pub const fn new(compute_pending_block: bool) -> Self {
        Self { compute_pending_block }
    }
}

impl<Node, Pool> PayloadServiceBuilder<Node, Pool> for OptimismPayloadBuilder
where
    Node: FullNodeTypes<Engine = OptimismEngineTypes>,
    Pool: TransactionPool + Unpin + 'static,
{
    async fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<Node::Engine>> {
        let payload_builder =
            reth_optimism_payload_builder::OptimismPayloadBuilder::new(ctx.chain_spec())
                .set_compute_pending_block(self.compute_pending_block);
        let conf = ctx.payload_builder_config();

        let payload_job_config = BasicPayloadJobGeneratorConfig::default()
            .interval(conf.interval())
            .deadline(conf.deadline())
            .max_payload_tasks(conf.max_payload_tasks())
            // no extradata for OP
            .extradata(Default::default())
            .max_gas_limit(conf.max_gas_limit());

        let payload_generator = BasicPayloadJobGenerator::with_builder(
            ctx.provider().clone(),
            pool,
            ctx.task_executor().clone(),
            payload_job_config,
            ctx.chain_spec(),
            payload_builder,
        );
        let (payload_service, payload_builder) =
            PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

        ctx.task_executor().spawn_critical("payload builder service", Box::pin(payload_service));

        Ok(payload_builder)
    }
}

/// A basic optimism network builder.
#[derive(Debug, Default, Clone)]
pub struct OptimismNetworkBuilder {
    /// HTTP endpoint for the sequencer mempool
    pub sequencer_http: Option<String>,
    /// Disable transaction pool gossip
    pub disable_txpool_gossip: bool,
}

impl<Node, Pool> NetworkBuilder<Node, Pool> for OptimismNetworkBuilder
where
    Node: FullNodeTypes,
    Pool: TransactionPool + Unpin + 'static,
{
    async fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<NetworkHandle> {
        let Self { sequencer_http, disable_txpool_gossip } = self;
        let mut network_config = ctx.network_config()?;

        // When `sequencer_endpoint` is configured, the node will forward all transactions to a
        // Sequencer node for execution and inclusion on L1, and disable its own txpool
        // gossip to prevent other parties in the network from learning about them.
        network_config.tx_gossip_disabled = disable_txpool_gossip;
        network_config.optimism_network_config.sequencer_endpoint = sequencer_http;

        let network = NetworkManager::builder(network_config).await?;

        let handle = ctx.start_network(network, pool);

        Ok(handle)
    }
}
