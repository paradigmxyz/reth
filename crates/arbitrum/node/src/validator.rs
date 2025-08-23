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
use reth_primitives_traits::{NodePrimitives, RecoveredBlock, Block};
use reth_provider::StateProviderFactory;
use reth_tracing::tracing;

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
        tracing::info!(target: "arb-reth::engine", %expected_hash, "arb engine: start ensure_well_formed_payload");
        let txs_len = exec_data.payload.as_v1().transactions.len();
        let has_pbbr = exec_data.sidecar.parent_beacon_block_root.is_some();
        tracing::info!(target: "arb-reth::engine", txs_len, has_pbbr, "arb engine: arb payload summary before decode");

        let sealed_block = {
            exec_data
                .try_into_block_with_sidecar(&exec_data.sidecar)
                .map_err(|e| {
                    tracing::warn!(target: "arb-reth::engine", err=%e, "arb engine: failed to decode arb payload");
                    NewPayloadError::Other(format!("failed to decode arb payload: {e}").into())
                })?
                .seal_slow()
        };

        let got_hash = sealed_block.hash();
        if expected_hash != got_hash {
            tracing::warn!(target: "arb-reth::engine", got=%got_hash, expected=%expected_hash, "arb engine: block hash mismatch");
            return Err(NewPayloadError::Other(format!(
                "block hash mismatch: execution={} consensus={}",
                got_hash,
                expected_hash
            ).into()));
        }

        let hdr = sealed_block.header();
        let base_fee = hdr.base_fee_per_gas;
        let gas_limit = hdr.gas_limit;
        let extra_len = hdr.extra_data.len();
        let parent = hdr.parent_hash;
        tracing::trace!(
            target: "arb-reth::engine",
            %parent,
            gas_limit=%gas_limit,
            base_fee=?base_fee,
            extra_len=%extra_len,
            "arb engine: decoded sealed block header details"
        );

        sealed_block
            .try_recover()
            .map_err(|e| {
                tracing::warn!(target: "arb-reth::engine", err=%e, "arb engine: failed to recover senders");
                NewPayloadError::Other(format!("failed to recover senders: {e}").into())
            })
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
        tracing::info!(
            target: "arb-reth::engine",
            ?_version,
            "arb engine: skipping version-specific withdrawals checks for Arbitrum"
        );
        Ok(())
    }

    fn ensure_well_formed_attributes(
        &self,
        _version: EngineApiMessageVersion,
        _attributes: &Types::PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        tracing::info!(
            target: "arb-reth::engine",
            ?_version,
            "arb engine: attributes well-formed (withdrawals ignored for Arbitrum)"
        );
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
