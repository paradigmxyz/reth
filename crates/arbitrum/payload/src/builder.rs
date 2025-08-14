use crate::ArbBuiltPayload;
use alloy_primitives::U256;
use reth_basic_payload_builder::{
    BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig, BuildArguments, BuildContext,
    BuildOutcome, BuildOutcomeKind, HeaderForPayload, PayloadBuilder, PayloadConfig,
    BuiltBlock, build_block,
};
use reth_chain_state::ExecutedBlockWithTrieUpdates;
use reth_chainspec::ChainSpecProvider;
use reth_evm::{execute::BlockExecutor, ConfigureEvm};
use reth_payload_builder::PayloadJobGenerator;
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::{BuildNextEnv, PayloadBuilderAttributes};
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_revm::{cached::CachedReads, database::StateProviderDatabase};
use reth_storage_api::StateProviderFactory;
use reth_payload_util::NoopPayloadTransactions;
use std::{marker::PhantomData, sync::Arc};

#[derive(Debug)]
pub struct ArbPayloadBuilder<Pool, Client, Evm, N, Attrs> {
    pub evm_config: Evm,
    pub pool: Pool,
    pub client: Client,
    _pd: PhantomData<(N, Attrs)>,
}

impl<Pool, Client, Evm, N, Attrs> Clone for ArbPayloadBuilder<Pool, Client, Evm, N, Attrs>
where
    Pool: Clone,
    Client: Clone,
    Evm: ConfigureEvm,
{
    fn clone(&self) -> Self {
        Self {
            evm_config: self.evm_config.clone(),
            pool: self.pool.clone(),
            client: self.client.clone(),
            _pd: PhantomData,
        }
    }
}

impl<Pool, Client, Evm, N, Attrs> ArbPayloadBuilder<Pool, Client, Evm, N, Attrs> {
    pub fn new(pool: Pool, client: Client, evm_config: Evm) -> Self {
        Self { pool, client, evm_config, _pd: PhantomData }
    }
}

impl<Pool, Client, Evm, N, Attrs> ArbPayloadBuilder<Pool, Client, Evm, N, Attrs>
where
    Pool: Send + Sync + Clone + 'static,
    Client: StateProviderFactory + ChainSpecProvider + Clone + Send + Sync + 'static,
    N: NodePrimitives,
    Evm: ConfigureEvm<
        Primitives = N,
        NextBlockEnvCtx: BuildNextEnv<Attrs, N::BlockHeader, Client::ChainSpec>,
    >,
    Attrs: PayloadBuilderAttributes + Clone,
{
    fn build_payload_internal(
        &self,
        args: BuildArguments<Attrs, ArbBuiltPayload<N>>,
    ) -> Result<BuildOutcome<ArbBuiltPayload<N>>, PayloadBuilderError> {
        let BuildArguments { mut cached_reads, config, cancel, best_payload } = args;

        let parent = config.parent_header.clone();
        let parent_hash = parent.hash();

        let ctx = BuildContext {
            evm_config: self.evm_config.clone(),
            chain_spec: self.client.chain_spec(),
            config,
            cancel,
            best_payload,
        };

        let state_provider = self.client.state_by_block_hash(parent_hash)?;
        let state_db = StateProviderDatabase::new(&state_provider);

        let block_builder = ctx.block_builder::<BlockExecutor<Evm::Primitives, _>>(&state_provider)?;

        let BuiltBlock { executed, payload } =
            build_block::<_, _, ArbBuiltPayload<N>, _, _>(
                block_builder,
                &state_provider,
                &state_db,
                &ctx,
                |_best_attrs| {
                    NoopPayloadTransactions::<()> ::default()
                },
            )?;

        let executed_block: Option<ExecutedBlockWithTrieUpdates<N>> = executed.map(|e| e.into());
        let payload = ArbBuiltPayload::new(ctx.payload_id(), Arc::new(payload), U256::ZERO, executed_block);

        Ok(BuildOutcomeKind::Better { payload }.with_cached_reads(cached_reads))
    }
}

impl<Pool, Client, Evm, N, Attrs> PayloadBuilder for ArbPayloadBuilder<Pool, Client, Evm, N, Attrs>
where
    Pool: Send + Sync + Clone + 'static,
    Client: StateProviderFactory + ChainSpecProvider + Clone + Send + Sync + 'static,
    N: NodePrimitives,
    Evm: ConfigureEvm<
        Primitives = N,
        NextBlockEnvCtx: BuildNextEnv<Attrs, N::BlockHeader, Client::ChainSpec>,
    >,
    Attrs: PayloadBuilderAttributes + Clone,
{
    type Primitives = N;
    type BuiltPayload = ArbBuiltPayload<N>;
    type Attributes = Attrs;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        self.build_payload_internal(args)
    }

    fn on_missing_payload(
        &self,
        _args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> reth_basic_payload_builder::MissingPayloadBehaviour<Self::BuiltPayload> {
        Default::default()
    }

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes, HeaderForPayload<Self::BuiltPayload>>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let args = BuildArguments::new(
            CachedReads::default(),
            config,
            Default::default(),
            Default::default(),
        );
        let out = self.build_payload_internal(args)?;
        Ok(out.into_payload())
    }
}

pub type ArbBasicPayloadJobGenerator<Client, Tasks, Builder> =
    BasicPayloadJobGenerator<Client, Tasks, Builder>;

pub fn arb_job_generator_with_builder<Client, Tasks, Builder>(
    client: Client,
    executor: Tasks,
    config: BasicPayloadJobGeneratorConfig,
    builder: Builder,
) -> ArbBasicPayloadJobGenerator<Client, Tasks, Builder> {
    BasicPayloadJobGenerator::with_builder(client, executor, config, builder)
}
