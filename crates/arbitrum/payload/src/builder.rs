use crate::ArbBuiltPayload;
use alloy_consensus::BlockHeader;
use alloy_primitives::U256;
use reth_basic_payload_builder::{
    BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig, BuildArguments, BuildOutcome,
    BuildOutcomeKind, HeaderForPayload, MissingPayloadBehaviour, PayloadBuilder, PayloadConfig,
};
use reth_chain_state::{ExecutedBlock, ExecutedBlockWithTrieUpdates, ExecutedTrieUpdates};
use reth_chainspec::ChainSpecProvider;
use reth_evm::{ConfigureEvm, execute::BlockBuilder};
use reth_execution_types::ExecutionOutcome;
use reth_payload_builder::PayloadJobGenerator;
use reth_payload_builder::PayloadBuilderError;
use reth_payload_primitives::{BuildNextEnv, PayloadBuilderAttributes};
use reth_primitives_traits::{HeaderTy, NodePrimitives, SealedHeader, SealedHeaderFor};
use reth_revm::{cached::CachedReads, cancelled::CancelOnDrop, database::StateProviderDatabase, db::State};
use reth_storage_api::{StateProvider, StateProviderFactory};
use std::{marker::PhantomData, sync::Arc};

#[derive(Debug)]
pub struct ArbPayloadBuilderCtx<Evm: ConfigureEvm, ChainSpec, Attrs> {
    pub evm_config: Evm,
    pub chain_spec: Arc<ChainSpec>,
    pub config: PayloadConfig<Attrs, HeaderTy<Evm::Primitives>>,
    pub cancel: CancelOnDrop,
    pub best_payload: Option<ArbBuiltPayload<Evm::Primitives>>,
}

impl<Evm, ChainSpec, Attrs> ArbPayloadBuilderCtx<Evm, ChainSpec, Attrs>
where
    Evm: ConfigureEvm,
    Evm::Primitives: NodePrimitives,
    Evm::NextBlockEnvCtx: BuildNextEnv<Attrs, HeaderTy<Evm::Primitives>, ChainSpec>,
    Attrs: PayloadBuilderAttributes,
{
    pub fn parent(&self) -> &SealedHeaderFor<Evm::Primitives> {
        self.config.parent_header.as_ref()
    }
    pub const fn attributes(&self) -> &Attrs {
        &self.config.attributes
    }
    pub fn payload_id(&self) -> alloy_rpc_types_engine::PayloadId {
        self.attributes().payload_id()
    }
    pub fn block_builder<'a, DB: reth_evm::Database>(
        &'a self,
        db: &'a mut State<DB>,
    ) -> Result<impl BlockBuilder<Primitives = Evm::Primitives> + 'a, PayloadBuilderError> {
        self.evm_config
            .builder_for_next_block(
                db,
                self.parent(),
                Evm::NextBlockEnvCtx::build_next_env(
                    self.attributes(),
                    self.parent(),
                    self.chain_spec.as_ref(),
                )
                .map_err(PayloadBuilderError::other)?,
            )
            .map_err(PayloadBuilderError::other)
    }
}


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

        let parent_hash = config.parent_header.hash();
        let ctx = ArbPayloadBuilderCtx {
            evm_config: self.evm_config.clone(),
            chain_spec: self.client.chain_spec(),
            config,
            cancel,
            best_payload,
        };

        let state_provider = self.client.state_by_block_hash(parent_hash)?;
        let mut db = State::builder()
            .with_database(StateProviderDatabase::new(&state_provider))
            .with_bundle_update()
            .build();

        let mut builder = ctx.block_builder(&mut db)?;

        builder.apply_pre_execution_changes().map_err(|err| PayloadBuilderError::Internal(err.into()))?;

        let outcome = builder.finish(&state_provider)?;
        let sealed_block = Arc::new(outcome.block.sealed_block().clone());

        let execution_outcome = ExecutionOutcome::new(
            db.take_bundle(),
            vec![outcome.execution_result.receipts],
            outcome.block.number(),
            Vec::new(),
        );

        let executed: ExecutedBlockWithTrieUpdates<N> = ExecutedBlockWithTrieUpdates {
            block: ExecutedBlock {
                recovered_block: Arc::new(outcome.block),
                execution_output: Arc::new(execution_outcome),
                hashed_state: Arc::new(outcome.hashed_state),
            },
            trie: ExecutedTrieUpdates::Present(Arc::new(outcome.trie_updates)),
        };

        let payload = ArbBuiltPayload::new(ctx.payload_id(), sealed_block, U256::ZERO, Some(executed));

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
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        MissingPayloadBehaviour::AwaitInProgress
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
        out.into_payload().ok_or(PayloadBuilderError::MissingPayload)
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
