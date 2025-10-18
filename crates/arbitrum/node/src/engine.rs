#![allow(unused)]

use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV2, ExecutionPayloadV1};
use reth_node_api::payload::PayloadTypes;
use reth_node_api::{BuiltPayload, EngineTypes, FullNodeComponents, NodeTypes};
use reth_primitives_traits::{NodePrimitives, SealedBlock};
use reth_provider::StateProviderFactory;
use std::marker::PhantomData;

use reth_arbitrum_payload::ArbExecutionData;
use reth_engine_tree::tree::{BasicEngineValidator, TreeConfig};
use reth_node_builder::rpc::EngineValidatorBuilder;
use reth_evm::ConfigureEngineEvm;
use reth_chainspec::EthChainSpec;
use reth_node_builder::invalid_block_hook::InvalidBlockHookExt;
use reth_node_builder::rpc::PayloadValidatorBuilder;
use reth_node_api::AddOnsContext;
use reth_tracing::tracing;

#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct ArbEngineTypes<T: PayloadTypes> {
    _marker: PhantomData<T>,
}

impl<T: PayloadTypes<ExecutionData = ArbExecutionData>> EngineTypes for ArbEngineTypes<T>
where
    T::BuiltPayload: BuiltPayload
        + TryInto<ExecutionPayloadV1>
        + TryInto<ExecutionPayloadEnvelopeV2>,
{
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV4 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV5 = ExecutionPayloadEnvelopeV2;
}
impl<T> PayloadTypes for ArbEngineTypes<T>
where
    T: PayloadTypes<ExecutionData = ArbExecutionData>,
{
    type ExecutionData = T::ExecutionData;
    type BuiltPayload = T::BuiltPayload;
    type PayloadAttributes = T::PayloadAttributes;
    type PayloadBuilderAttributes = T::PayloadBuilderAttributes;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> Self::ExecutionData {
        <T as PayloadTypes>::block_to_payload(block)
    }
}

impl<N> PayloadValidatorBuilder<N> for ArbEngineValidatorBuilder
where
    N: FullNodeComponents<
        Types: NodeTypes<
            Payload: PayloadTypes<ExecutionData = ArbExecutionData>,
            Primitives: NodePrimitives<
                SignedTx = reth_arbitrum_primitives::ArbTransactionSigned,
                Block = alloy_consensus::Block<reth_arbitrum_primitives::ArbTransactionSigned>,
            >,
        >,
        Provider: StateProviderFactory,
    >,
{
    type Validator = crate::validator::ArbEngineValidator<N::Provider>;

    async fn build(self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::Validator> {
        Ok(crate::validator::ArbEngineValidator::new(ctx.node.provider().clone()))
    }
}


#[derive(Debug, Default, Clone, Copy)]
pub struct ArbEngineValidatorBuilder;

impl<N> EngineValidatorBuilder<N> for ArbEngineValidatorBuilder
where
    N: FullNodeComponents<
        Types: NodeTypes<
            Payload: PayloadTypes<ExecutionData = ArbExecutionData>,
            Primitives: NodePrimitives<
                SignedTx = reth_arbitrum_primitives::ArbTransactionSigned,
                Block = alloy_consensus::Block<reth_arbitrum_primitives::ArbTransactionSigned>,
            >,
        >,
        Provider: StateProviderFactory,
        Evm: ConfigureEngineEvm<ArbExecutionData>,
    >,
{
    type EngineValidator = BasicEngineValidator<
        N::Provider,
        N::Evm,
        crate::validator::ArbEngineValidator<N::Provider>,
    >;

    async fn build_tree_validator(
        self,
        ctx: &reth_node_api::AddOnsContext<'_, N>,
        mut tree_config: TreeConfig,
    ) -> eyre::Result<Self::EngineValidator> {
        tree_config = tree_config
            .with_state_root_fallback(true)
            .with_disable_parallel_sparse_trie(true)
            .with_legacy_state_root(true);

        tracing::info!(
            target: "arb-reth::engine",
            "arb engine: building tree validator with legacy_state_root=true, state_root_fallback=true, parallel_sparse_trie=false"
        );

        tracing::info!(
            target: "arb-reth::engine",
            "arb engine: withdrawals presence checks disabled via ArbEngineValidator"
        );
        let payload_validator = crate::validator::ArbEngineValidator::new(ctx.node.provider().clone());
        let data_dir = ctx.config.datadir.clone().resolve_datadir(ctx.config.chain.chain());
        let invalid_block_hook = ctx.create_invalid_block_hook(&data_dir).await?;
        Ok(BasicEngineValidator::new(
            ctx.node.provider().clone(),
            std::sync::Arc::new(ctx.node.consensus().clone()),
            ctx.node.evm_config().clone(),
            payload_validator,
            tree_config,
            invalid_block_hook,
        ))
    }
}
