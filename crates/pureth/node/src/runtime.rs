use crate::{
    ExecutionConsensus, ExecutionEngineTypes, ExecutionEngineValidator, ExecutionEvmConfig,
    ExecutionPayloadBuilder, ExecutionSourceBackend, ExecutionStorage,
};
use alloy_rpc_types_engine::ClientVersionV1;
use jsonrpsee_core::server::RpcModule;
use reth_chainspec::{ChainSpec, EthChainSpec};
use reth_db_api::{database_metrics::DatabaseMetrics, Database};
use reth_ethereum_payload_builder::EthereumBuilderConfig;
use reth_evm_ethereum::EthEvmConfig;
use reth_node_api::{
    AddOnsContext, FullNodeComponents, FullNodeTypes, NodeTypes, NodeTypesWithDBAdapter,
};
use reth_node_builder::{
    components::{
        BasicPayloadServiceBuilder, ComponentsBuilder, ConsensusBuilder, ExecutorBuilder,
        PayloadBuilderBuilder,
    },
    rpc::{
        BasicEngineValidatorBuilder, EngineApiBuilder, EthApiBuilder, EthApiCtx,
        PayloadValidatorBuilder, RpcAddOns,
    },
    BuilderContext, Node, NodeAdapter, PayloadBuilderConfig,
};
use reth_node_core::version::{version_metadata, CLIENT_CODE};
use reth_node_ethereum::node::{EthereumNetworkBuilder, EthereumPoolBuilder};
use reth_payload_builder::PayloadStore;
use reth_provider::{providers::BlockchainProvider, ChainSpecProvider};
use reth_pureth_block::{
    ExecutionHeader, ExecutionPrimitives, ExecutionSourceProvider, PayloadLimits,
    PayloadSourceError,
};
use reth_pureth_query::{
    ExecutionHeaderConverter, ExecutionReceiptConverter, ExecutionRpcTypes, PurethApiServer,
    PurethRpc, QueryService, SelectionLimits,
};
use reth_rpc::EthApi;
use reth_rpc_api::IntoEngineApiRpcModule;
use reth_rpc_convert::RpcConverter;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use std::{
    num::NonZeroU32,
    sync::{Arc, OnceLock},
};

#[derive(Clone, Debug)]
pub struct ExecutionNode {
    validator: ExecutionEngineValidator,
    retained_entries: NonZeroU32,
    sources: Arc<OnceLock<Arc<dyn ExecutionSourceProvider>>>,
}

impl ExecutionNode {
    pub fn new(
        chain_spec: Arc<ChainSpec>,
        retained_entries: NonZeroU32,
    ) -> Result<Self, PayloadSourceError> {
        let validator = ExecutionEngineValidator::new(chain_spec, PayloadLimits::default())?;
        if validator.fork_config().is_active_at_timestamp(validator.chain_spec().genesis.timestamp)
        {
            return Err(PayloadSourceError::UnsupportedFork);
        }
        Ok(Self { validator, retained_entries, sources: Arc::new(OnceLock::new()) })
    }

    pub fn source_provider(&self) -> Option<&Arc<dyn ExecutionSourceProvider>> {
        self.sources.get()
    }
}

impl NodeTypes for ExecutionNode {
    type Primitives = ExecutionPrimitives;
    type ChainSpec = ChainSpec<ExecutionHeader>;
    type Storage = ExecutionStorage;
    type Payload = ExecutionEngineTypes;
}

type ExecutionRpcConverter = RpcConverter<
    ExecutionRpcTypes,
    ExecutionEvmConfig,
    ExecutionReceiptConverter<ChainSpec<ExecutionHeader>>,
    ExecutionHeaderConverter,
>;

#[derive(Debug, Default)]
pub struct ExecutionEthApiBuilder(Option<ExecutionNode>);

impl<N, DB> EthApiBuilder<N> for ExecutionEthApiBuilder
where
    N: FullNodeComponents<
        Types = ExecutionNode,
        DB = DB,
        Provider = BlockchainProvider<NodeTypesWithDBAdapter<ExecutionNode, DB>>,
        Evm = ExecutionEvmConfig,
    >,
    DB: Database + DatabaseMetrics + Clone + Unpin + 'static,
{
    type EthApi = EthApi<N, ExecutionRpcConverter>;

    async fn build_eth_api(self, ctx: EthApiCtx<'_, N>) -> eyre::Result<Self::EthApi> {
        let node = self.0.ok_or_else(|| eyre::eyre!("execution node configuration missing"))?;
        let source = Arc::new(ExecutionSourceBackend::new(
            ctx.components.provider().clone(),
            node.validator.clone(),
            node.retained_entries,
        )?);
        node.sources
            .set(source)
            .map_err(|_| eyre::eyre!("execution source backend already installed"))?;
        let converter = RpcConverter::new(ExecutionReceiptConverter::new(
            ctx.components.provider().chain_spec(),
            node.validator.fork_config(),
            SelectionLimits::default(),
        ))
        .with_header_converter(ExecutionHeaderConverter::new(node.validator.fork_config()));
        Ok(ctx.eth_api_builder().with_rpc_converter(converter).build())
    }
}

impl<N, DB> Node<N> for ExecutionNode
where
    N: FullNodeTypes<
        Types = Self,
        DB = DB,
        Provider = BlockchainProvider<NodeTypesWithDBAdapter<Self, DB>>,
    >,
    DB: Database + DatabaseMetrics + Clone + Unpin + 'static,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        BasicPayloadServiceBuilder<Self>,
        EthereumNetworkBuilder,
        Self,
        Self,
    >;
    type AddOns = RpcAddOns<NodeAdapter<N>, ExecutionEthApiBuilder, Self, Self>;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(EthereumPoolBuilder::default())
            .executor(self.clone())
            .payload(BasicPayloadServiceBuilder::new(self.clone()))
            .network(EthereumNetworkBuilder::default())
            .consensus(self.clone())
    }

    fn add_ons(&self) -> Self::AddOns {
        let node = self.clone();
        RpcAddOns::new(
            ExecutionEthApiBuilder(Some(self.clone())),
            self.clone(),
            self.clone(),
            BasicEngineValidatorBuilder::new(self.clone()),
            Default::default(),
            Default::default(),
        )
        .extend_rpc_modules(move |ctx| {
            let source = node
                .source_provider()
                .ok_or_else(|| eyre::eyre!("execution source backend not installed"))?
                .clone();
            ctx.modules.merge_configured(
                PurethRpc::new(QueryService::from_execution_source(source)).into_rpc(),
            )?;
            Ok(())
        })
    }
}

impl<N: FullNodeTypes<Types = Self>> ExecutorBuilder<N> for ExecutionNode {
    type EVM = ExecutionEvmConfig;

    async fn build_evm(self, ctx: &BuilderContext<N>) -> eyre::Result<Self::EVM> {
        self.validator.validate_chain_spec(ctx.chain_spec().as_ref())?;
        let mut native = EthEvmConfig::new(self.validator.chain_spec().clone());
        if let Some(cache) = ctx.sender_recovery_cache() {
            native = native.with_sender_recovery_cache(cache.clone());
        }
        Ok(ExecutionEvmConfig::new(native, self.validator)?)
    }
}

impl<N: FullNodeTypes<Types = Self>> ConsensusBuilder<N> for ExecutionNode {
    type Consensus = Arc<ExecutionConsensus>;

    async fn build_consensus(self, ctx: &BuilderContext<N>) -> eyre::Result<Self::Consensus> {
        self.validator.validate_chain_spec(ctx.chain_spec().as_ref())?;
        Ok(Arc::new(ExecutionConsensus::new(self.validator)))
    }
}

impl<N, Pool> PayloadBuilderBuilder<N, Pool, ExecutionEvmConfig> for ExecutionNode
where
    N: FullNodeTypes<Types = Self>,
    Pool: TransactionPool<
            Transaction: PoolTransaction<Consensus = reth_ethereum_primitives::TransactionSigned>,
        > + Unpin
        + 'static,
{
    type PayloadBuilder = ExecutionPayloadBuilder<Pool, N::Provider, EthEvmConfig>;

    async fn build_payload_builder(
        self,
        ctx: &BuilderContext<N>,
        pool: Pool,
        evm: ExecutionEvmConfig,
    ) -> eyre::Result<Self::PayloadBuilder> {
        let conf = ctx.payload_builder_config();
        let config = EthereumBuilderConfig::new()
            .with_gas_limit(conf.gas_limit_for(ctx.chain_spec().chain()))
            .with_max_blobs_per_block(conf.max_blobs_per_block())
            .with_extra_data(conf.extra_data())
            .with_skip_state_root(ctx.config().tree_config().skip_state_root());
        Ok(ExecutionPayloadBuilder::from_validator(
            ctx.provider().clone(),
            pool,
            evm.native_config().clone(),
            config,
            &self.validator,
        )?)
    }
}

impl<N: FullNodeComponents<Types = Self>> PayloadValidatorBuilder<N> for ExecutionNode {
    type Validator = ExecutionEngineValidator;

    async fn build(self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::Validator> {
        self.validator.validate_chain_spec(ctx.config.chain.as_ref())?;
        Ok(self.validator)
    }
}

#[derive(Debug)]
pub struct ExecutionEngineRpc(RpcModule<()>);

impl IntoEngineApiRpcModule for ExecutionEngineRpc {
    fn into_rpc_module(self) -> RpcModule<()> {
        self.0
    }
}

impl<N: FullNodeComponents<Types = Self>> EngineApiBuilder<N> for ExecutionNode {
    type EngineApi = ExecutionEngineRpc;

    async fn build_engine_api(self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::EngineApi> {
        self.validator.validate_chain_spec(ctx.config.chain.as_ref())?;
        eyre::ensure!(
            !ctx.config.engine.accept_execution_requests_hash,
            "full execution requests are required"
        );
        let metadata = version_metadata();
        let client = ClientVersionV1 {
            code: CLIENT_CODE,
            name: metadata.name_client.to_string(),
            version: metadata.cargo_pkg_version.to_string(),
            commit: metadata.vergen_git_sha.to_string(),
        };
        Ok(ExecutionEngineRpc(self.validator.into_rpc(
            ctx.node.provider().clone(),
            ctx.beacon_engine_handle.clone(),
            PayloadStore::new(ctx.node.payload_builder_handle().clone()),
            ctx.node.pool().clone(),
            ctx.node.task_executor().clone(),
            client,
            ctx.node.network().clone(),
        )))
    }
}
