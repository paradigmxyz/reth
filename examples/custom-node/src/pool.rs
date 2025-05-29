// use jsonrpsee::tracing::{debug, info};
use crate::primitives::CustomTransactionEnvelope;
use op_alloy_consensus::{interop::SafetyLevel, OpTxEnvelope};
use reth_chain_state::CanonStateSubscriptions;
use reth_node_builder::{
    components::{PoolBuilder, PoolBuilderConfigOverrides},
    node::{FullNodeTypes, NodeTypes},
    BuilderContext, NodePrimitives,
};
use reth_op::{
    node::txpool::{
        supervisor::{SupervisorClient, DEFAULT_SUPERVISOR_URL},
        OpPooledTransaction, OpPooledTx, OpTransactionPool, OpTransactionValidator,
    },
    pool::{
        blobstore::DiskFileBlobStore, CoinbaseTipOrdering, EthPoolTransaction,
        TransactionValidationTaskExecutor,
    },
    primitives::Extended,
};
use reth_optimism_forks::OpHardforks;

#[derive(Debug, Clone)]
pub struct CustomPoolBuilder<
    T = OpPooledTransaction<
        Extended<OpTxEnvelope, CustomTransactionEnvelope>,
        Extended<op_alloy_consensus::OpPooledTransaction, CustomTransactionEnvelope>,
    >,
> {
    /// Enforced overrides that are applied to the pool config.
    pub pool_config_overrides: PoolBuilderConfigOverrides,
    /// Enable transaction conditionals.
    pub enable_tx_conditional: bool,
    /// Supervisor client url
    pub supervisor_http: String,
    /// Supervisor safety level
    pub supervisor_safety_level: SafetyLevel,
    /// Marker for the pooled transaction type.
    _pd: core::marker::PhantomData<T>,
}

impl<T> Default for CustomPoolBuilder<T> {
    fn default() -> Self {
        Self {
            pool_config_overrides: Default::default(),
            enable_tx_conditional: false,
            supervisor_http: DEFAULT_SUPERVISOR_URL.to_string(),
            supervisor_safety_level: SafetyLevel::CrossUnsafe,
            _pd: Default::default(),
        }
    }
}

impl<T> CustomPoolBuilder<T> {
    /// Sets the enable_tx_conditional flag on the pool builder.
    pub const fn with_enable_tx_conditional(mut self, enable_tx_conditional: bool) -> Self {
        self.enable_tx_conditional = enable_tx_conditional;
        self
    }

    /// Sets the [PoolBuilderConfigOverrides] on the pool builder.
    pub fn with_pool_config_overrides(
        mut self,
        pool_config_overrides: PoolBuilderConfigOverrides,
    ) -> Self {
        self.pool_config_overrides = pool_config_overrides;
        self
    }

    /// Sets the supervisor client
    pub fn with_supervisor(
        mut self,
        supervisor_client: String,
        supervisor_safety_level: SafetyLevel,
    ) -> Self {
        self.supervisor_http = supervisor_client;
        self.supervisor_safety_level = supervisor_safety_level;
        self
    }
}

impl<Node, T> PoolBuilder<Node> for CustomPoolBuilder<T>
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec: OpHardforks>>,
    <Node::Types as NodeTypes>::Primitives:
        NodePrimitives<SignedTx = Extended<OpTxEnvelope, CustomTransactionEnvelope>>,
    T: EthPoolTransaction<Consensus = Extended<OpTxEnvelope, CustomTransactionEnvelope>>
        + OpPooledTx,
{
    type Pool = OpTransactionPool<Node::Provider, DiskFileBlobStore, T>;

    async fn build_pool(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        let Self { pool_config_overrides, .. } = self;
        let data_dir = ctx.config().datadir();
        let blob_store = DiskFileBlobStore::open(data_dir.blobstore(), Default::default())?;
        // supervisor used for interop
        if ctx.chain_spec().is_interop_active_at_timestamp(ctx.head().timestamp) &&
            self.supervisor_http == DEFAULT_SUPERVISOR_URL
        {
            // info!(target: "reth::cli",
            //     url=%DEFAULT_SUPERVISOR_URL,
            //     "Default supervisor url is used, consider changing --rollup.supervisor-http."
            // );
        }
        let supervisor_client = SupervisorClient::builder(self.supervisor_http.clone())
            .minimum_safety(self.supervisor_safety_level)
            .build()
            .await;

        let validator = TransactionValidationTaskExecutor::eth_builder(ctx.provider().clone())
            .no_eip4844()
            .with_head_timestamp(ctx.head().timestamp)
            .kzg_settings(ctx.kzg_settings()?)
            .set_tx_fee_cap(ctx.config().rpc.rpc_tx_fee_cap)
            .with_additional_tasks(
                pool_config_overrides
                    .additional_validation_tasks
                    .unwrap_or_else(|| ctx.config().txpool.additional_validation_tasks),
            )
            .build_with_tasks(ctx.task_executor().clone(), blob_store.clone())
            .map(|validator| {
                OpTransactionValidator::new(validator)
                    // In --dev mode we can't require gas fees because we're unable to decode
                    // the L1 block info
                    .require_l1_data_gas_fee(!ctx.config().dev.dev)
                    .with_supervisor(supervisor_client.clone())
            });

        let transaction_pool = reth_ethereum::pool::Pool::new(
            validator,
            CoinbaseTipOrdering::default(),
            blob_store,
            pool_config_overrides.apply(ctx.pool_config()),
        );
        // info!(target: "reth::cli", "Transaction pool initialized";);

        // spawn txpool maintenance tasks
        {
            let pool = transaction_pool.clone();
            let chain_events = ctx.provider().canonical_state_stream();
            let client = ctx.provider().clone();
            if !ctx.config().txpool.disable_transactions_backup {
                // Use configured backup path or default to data dir
                let transactions_path = ctx
                    .config()
                    .txpool
                    .transactions_backup_path
                    .clone()
                    .unwrap_or_else(|| data_dir.txpool_transactions());

                let transactions_backup_config =
                    reth_ethereum::pool::maintain::LocalTransactionBackupConfig::with_local_txs_backup(transactions_path);

                ctx.task_executor().spawn_critical_with_graceful_shutdown_signal(
                    "local transactions backup task",
                    |shutdown| {
                        reth_ethereum::pool::maintain::backup_local_transactions_task(
                            shutdown,
                            pool.clone(),
                            transactions_backup_config,
                        )
                    },
                );
            }

            // spawn the main maintenance task
            ctx.task_executor().spawn_critical(
                "txpool maintenance task",
                reth_ethereum::pool::maintain::maintain_transaction_pool_future(
                    client,
                    pool.clone(),
                    chain_events,
                    ctx.task_executor().clone(),
                    reth_ethereum::pool::maintain::MaintainPoolConfig {
                        max_tx_lifetime: pool.config().max_queued_lifetime,
                        no_local_exemptions: transaction_pool
                            .config()
                            .local_transactions_config
                            .no_exemptions,
                        ..Default::default()
                    },
                ),
            );
            // debug!(target: "reth::cli", "Spawned txpool maintenance task");

            // spawn the Op txpool maintenance task
            let chain_events = ctx.provider().canonical_state_stream();
            ctx.task_executor().spawn_critical(
                "Op txpool interop maintenance task",
                reth_op::node::txpool::maintain::maintain_transaction_pool_interop_future(
                    pool.clone(),
                    chain_events,
                    supervisor_client,
                ),
            );
            // debug!(target: "reth::cli", "Spawned Op interop txpool maintenance task");

            if self.enable_tx_conditional {
                // spawn the Op txpool maintenance task
                let chain_events = ctx.provider().canonical_state_stream();
                ctx.task_executor().spawn_critical(
                    "Op txpool conditional maintenance task",
                    reth_op::node::txpool::maintain::maintain_transaction_pool_conditional_future(
                        pool,
                        chain_events,
                    ),
                );
                // debug!(target: "reth::cli", "Spawned Op conditional txpool maintenance task");
            }
        }

        Ok(transaction_pool)
    }
}
