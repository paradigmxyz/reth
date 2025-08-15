use reth_arbitrum_payload::ArbExecutionData;
use reth_engine_primitives::EngineApiValidator;
use reth_engine_primitives::PayloadValidator;
use reth_payload_primitives::{
    EngineApiMessageVersion,
    EngineObjectValidationError,
    NewPayloadError,
    PayloadOrAttributes,
    ExecutionPayload,
};
use reth_node_api::{EngineTypes, FullNodeComponents, NodeTypes, PayloadTypes};
use reth_primitives_traits::{NodePrimitives, RecoveredBlock};
use reth_provider::StateProviderFactory;

#[derive(Debug, Clone)]
pub struct ArbEngineValidator<P> {
    provider: P,
}

impl<P> ArbEngineValidator<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

impl<Types, P> PayloadValidator<Types> for ArbEngineValidator<P>
where
    Types: PayloadTypes<ExecutionData = ArbExecutionData, BuiltPayload: BuiltPayloadPrimsHelper>,
    <Types::BuiltPayload as BuiltPayloadPrimsHelper>::Prims: NodePrimitives<
        SignedTx = reth_arbitrum_primitives::ArbTransactionSigned,
        Block = alloy_consensus::Block<reth_arbitrum_primitives::ArbTransactionSigned>,
    >,
    P: StateProviderFactory + Send + Sync + Unpin + 'static,
{
    type Block = alloy_consensus::Block<reth_arbitrum_primitives::ArbTransactionSigned>;

    fn ensure_well_formed_payload(
        &self,
        exec_data: ArbExecutionData,
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
        let expected_hash = exec_data.block_hash();

        let sealed_block = {
            exec_data
                .try_into_block_with_sidecar(&exec_data.sidecar)
                .map_err(|e| NewPayloadError::Other(format!("failed to decode arb payload: {e}").into()))?
                .seal_slow()
        };

        if expected_hash != sealed_block.hash() {
            return Err(NewPayloadError::Other(format!(
                "block hash mismatch: execution={} consensus={}",
                sealed_block.hash(),
                expected_hash
            ).into()));
        }

        sealed_block
            .try_recover()
            .map_err(|e| NewPayloadError::Other(format!("failed to recover senders: {e}").into()))
    }
}

impl<Types, P> EngineApiValidator<Types> for ArbEngineValidator<P>
where
    Types: PayloadTypes<ExecutionData = ArbExecutionData>,
    P: StateProviderFactory + Send + Sync + Unpin + 'static,
{
    fn validate_version_specific_fields(
        &self,
        _version: EngineApiMessageVersion,
        _payload_or_attrs: PayloadOrAttributes<'_, Types::ExecutionData, Types::PayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        Ok(())
    }

    fn ensure_well_formed_attributes(
        &self,
        _version: EngineApiMessageVersion,
        _attributes: &Types::PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        Ok(())
    }
}

pub trait BuiltPayloadPrimsHelper: reth_node_api::BuiltPayload {
    type Prims: NodePrimitives;
}
impl<T: reth_node_api::BuiltPayload> BuiltPayloadPrimsHelper for T {
    type Prims = <T as reth_node_api::BuiltPayload>::Primitives;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArbPayloadValidatorBuilder;

impl<N> reth_node_builder::rpc::PayloadValidatorBuilder<N> for ArbPayloadValidatorBuilder
where
    N: FullNodeComponents<
        Types: NodeTypes<
            Payload: EngineTypes<ExecutionData = ArbExecutionData>,
            Primitives: NodePrimitives<
                SignedTx = reth_arbitrum_primitives::ArbTransactionSigned,
                Block = alloy_consensus::Block<reth_arbitrum_primitives::ArbTransactionSigned>,
            >,
        >,
        Provider: StateProviderFactory,
    >,
{
    type Validator = ArbEngineValidator<N::Provider>;

    async fn build(
        self,
        ctx: &reth_node_api::AddOnsContext<'_, N>,
    ) -> eyre::Result<Self::Validator> {
        Ok(ArbEngineValidator::new(ctx.node.provider().clone()))
    }
}
