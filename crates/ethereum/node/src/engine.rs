//! Validates execution payload wrt Ethereum Execution Engine API version.

use alloy_rpc_types_engine::ExecutionData;
pub use alloy_rpc_types_engine::{
    ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4,
    ExecutionPayloadV1, PayloadAttributes as EthPayloadAttributes,
};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_engine_primitives::{EngineApiValidator, PayloadValidator};
use reth_ethereum_payload_builder::EthereumExecutionPayloadValidator;
use reth_ethereum_primitives::Block;
use reth_node_api::PayloadTypes;
use reth_payload_primitives::{
    validate_execution_requests, validate_version_specific_fields, EngineApiMessageVersion,
    EngineObjectValidationError, NewPayloadError, PayloadOrAttributes,
};
use reth_primitives_traits::SealedBlock;
use std::sync::Arc;

/// Validator for the ethereum engine API.
#[derive(Debug, Clone)]
pub struct EthereumEngineValidator<ChainSpec = reth_chainspec::ChainSpec> {
    inner: EthereumExecutionPayloadValidator<ChainSpec>,
}

impl<ChainSpec> EthereumEngineValidator<ChainSpec> {
    /// Instantiates a new validator.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { inner: EthereumExecutionPayloadValidator::new(chain_spec) }
    }

    /// Returns the chain spec used by the validator.
    #[inline]
    fn chain_spec(&self) -> &ChainSpec {
        self.inner.chain_spec()
    }
}

impl<ChainSpec, Types> PayloadValidator<Types> for EthereumEngineValidator<ChainSpec>
where
    ChainSpec: EthChainSpec + EthereumHardforks + 'static,
    Types: PayloadTypes<ExecutionData = ExecutionData>,
{
    type Block = Block;

    fn convert_payload_to_block(
        &self,
        payload: ExecutionData,
    ) -> Result<SealedBlock<Self::Block>, NewPayloadError> {
        self.inner.ensure_well_formed_payload(payload).map_err(Into::into)
    }

    fn payload_transactions_root_is_validated(&self) -> bool {
        true
    }
}

impl<ChainSpec, Types> EngineApiValidator<Types> for EthereumEngineValidator<ChainSpec>
where
    ChainSpec: EthChainSpec + EthereumHardforks + 'static,
    Types: PayloadTypes<PayloadAttributes = EthPayloadAttributes, ExecutionData = ExecutionData>,
{
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, Types::ExecutionData, EthPayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        payload_or_attrs
            .execution_requests()
            .map(|requests| validate_execution_requests(requests))
            .transpose()?;

        validate_version_specific_fields(self.chain_spec(), version, payload_or_attrs)
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &EthPayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(
            self.chain_spec(),
            version,
            PayloadOrAttributes::<Types::ExecutionData, EthPayloadAttributes>::PayloadAttributes(
                attributes,
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{hex, Bytes};
    use alloy_rpc_types_engine::ExecutionPayload;
    use reth_chainspec::ChainSpecBuilder;
    use reth_ethereum_engine_primitives::EthPayloadTypes;

    #[test]
    fn validates_transaction_root_during_payload_conversion() {
        let validator = EthereumEngineValidator::new(Arc::new(ChainSpecBuilder::mainnet().build()));
        assert!(PayloadValidator::<EthPayloadTypes>::payload_transactions_root_is_validated(
            &validator,
        ));

        let block = Block::default();
        let mut payload = ExecutionData::from_block_unchecked(block.header.hash_slow(), &block);
        let ExecutionPayload::V1(payload_v1) = &mut payload.payload else {
            panic!("default block should produce a V1 payload");
        };
        // Adding a transaction without updating the advertised block hash makes the payload's
        // transactions inconsistent with the header derived from them.
        payload_v1.transactions.push(Bytes::from(hex::decode(
            "02f876820a28808477359400847735940082520894ab0840c0e43688012c1adb0f5e3fc665188f83d28a029d394a5d630544000080c080a0a044076b7e67b5deecc63f61a8d7913fab86ca365b344b5759d1fe3563b4c39ea019eab979dd000da04dfc72bb0377c092d30fd9e1cab5ae487de49586cc8b0090",
        ).unwrap()));

        assert!(PayloadValidator::<EthPayloadTypes>::convert_payload_to_block(&validator, payload)
            .is_err());
    }
}
