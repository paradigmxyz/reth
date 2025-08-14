use crate::{ArbBuiltPayload, ArbPayloadTypes};
use alloy_primitives::U256;
use reth_payload_basic::{
    BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig, HeaderForPayload, PayloadBuilder,
    PayloadConfig,
};
use reth_chain_state::ExecutedBlockWithTrieUpdates;
use reth_evm::{execute::BlockExecutor, ConfigureEvm};
use reth_payload_builder::{PayloadBuilderError, PayloadJobGenerator};
use reth_payload_primitives::{BuildNextEnv, BuiltPayload, PayloadBuilderAttributes};
use reth_primitives_traits::{HeaderTy, NodePrimitives, SealedHeader};
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_storage_api::StateProviderFactory;
use std::{marker::PhantomData, sync::Arc};

#[derive(Debug, Clone)]
pub struct ArbPayloadBuilder<Pool, Client, Evm, N, Attrs> {
    pub evm_config: Evm,
    pub pool: Pool,
    pub client: Client,
    _pd: PhantomData<(N, Attrs)>,
}

impl<Pool, Client, Evm, N, Attrs> ArbPayloadBuilder<Pool, Client, Evm, N, Attrs> {
    pub fn new(pool: Pool, client: Client, evm_config: Evm) -> Self {
        Self { pool, client, evm_config, _pd: PhantomData }
    }
}

impl<Pool, Client, Evm, N, Attrs> ArbPayloadBuilder<Pool, Client, Evm, N, Attrs>
where
    Client: StateProviderFactory + Clone + 'static,
    N: NodePrimitives,
    Evm: ConfigureEvm<
        Primitives = N,
        NextBlockEnvCtx: BuildNextEnv<Attrs, N::BlockHeader, Client::ChainSpec>,
    >,
    Attrs: PayloadBuilderAttributes,
{
    fn build_payload_internal(
        &self,
        args: reth_payload_basic::BuildArguments<Attrs, ArbBuiltPayload<N>>,
    ) -> Result<reth_payload_basic::BuildOutcome<ArbBuiltPayload<N>>, PayloadBuilderError> {
        let reth_payload_basic::BuildArguments { mut cached_reads, config, cancel, best_payload } = args;

        let parent = config.parent_header.clone();
        let parent_hash = parent.hash();

        let ctx = reth_payload_basic::BuildContext {
            evm_config: self.evm_config.clone(),
            chain_spec: self.client.chain_spec(),
            config,
            cancel,
            best_payload,
        };

        let state_provider = self.client.state_by_block_hash(parent_hash)?;
        let state_db = StateProviderDatabase::new(&state_provider);

        let block_builder = ctx.block_builder::<BlockExecutor<Evm::Primitives, _>>(&state_provider)?;

        let reth_payload_basic::BuiltBlock { executed, payload } =
            reth_payload_basic::build_block::<_, _, ArbBuiltPayload<N>, _, _>(
                block_builder,
                &state_provider,
                &state_db,
                &ctx,
                |best_attrs| {
                    reth_payload_util::NoopPayloadTransactions::default()
                        .best_transactions(best_attrs)
                },
            )?;

        let executed_block: Option<ExecutedBlockWithTrieUpdates<N>> = executed.map(|e| e.into());
        let payload = ArbBuiltPayload::new(ctx.payload_id(), Arc::new(payload), U256::ZERO, executed_block);

        Ok(reth_payload_basic::BuildOutcome {
            cached_reads,
            payload,
        })
    }
}

impl<Pool, Client, Evm, N, Attrs> PayloadBuilder for ArbPayloadBuilder<Pool, Client, Evm, N, Attrs>
where
    Client: StateProviderFactory + Clone + 'static,
    N: NodePrimitives,
    Evm: ConfigureEvm<
        Primitives = N,
        NextBlockEnvCtx: BuildNextEnv<Attrs, N::BlockHeader, Client::ChainSpec>,
    >,
    Attrs: PayloadBuilderAttributes,
{
    type Primitives = N;
    type BuiltPayload = ArbBuiltPayload<N>;
    type Attributes = Attrs;

    fn try_build(
        &self,
        args: reth_payload_basic::BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<reth_payload_basic::BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        self.build_payload_internal(args)
    }

    fn on_missing_payload(
        &self,
        _parent: SealedHeader<HeaderForPayload<Self::BuiltPayload>>,
        _attributes: Self::Attributes,
    ) -> Result<(), PayloadBuilderError> {
        Ok(())
    }

    fn build_empty_payload(
        &self,
        parent: SealedHeader<HeaderForPayload<Self::BuiltPayload>>,
        attributes: Self::Attributes,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let ctx = reth_payload_basic::PayloadConfig { parent_header: Arc::new(parent), attributes };
        let args = reth_payload_basic::BuildArguments {
            cached_reads: State::default(),
            config: ctx,
            cancel: Default::default(),
            best_payload: Default::default(),
        };
        let out = self.build_payload_internal(args)?;
        Ok(out.payload)
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

pub type ArbPayloadBuilderFor<Node, Pool, Evm> = reth_ethereum_payload_builder::EthereumPayloadBuilder<Pool, <Node as reth_node_api::FullNodeTypes>::Provider, Evm>;
