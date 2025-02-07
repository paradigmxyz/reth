//! Ethereum specific engine API types and impls.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod payload;
use alloc::sync::Arc;
use alloy_rpc_types_engine::{ExecutionPayload, PayloadError};
pub use alloy_rpc_types_engine::{
    ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4,
    ExecutionPayloadV1, PayloadAttributes as EthPayloadAttributes,
};
pub use payload::{EthBuiltPayload, EthPayloadBuilderAttributes};
use reth_chainspec::ChainSpec;
use reth_engine_primitives::{EngineTypes, EngineValidator, ExecutionData, PayloadValidator};
use reth_payload_primitives::{
    validate_version_specific_fields, BuiltPayload, EngineApiMessageVersion,
    EngineObjectValidationError, PayloadOrAttributes, PayloadTypes,
};
use reth_payload_validator::ExecutionPayloadValidator;
use reth_primitives::{Block, NodePrimitives, SealedBlock};

/// The types used in the default mainnet ethereum beacon consensus engine.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct EthEngineTypes<T: PayloadTypes = EthPayloadTypes> {
    _marker: core::marker::PhantomData<T>,
}

impl<T: PayloadTypes> PayloadTypes for EthEngineTypes<T> {
    type BuiltPayload = T::BuiltPayload;
    type PayloadAttributes = T::PayloadAttributes;
    type PayloadBuilderAttributes = T::PayloadBuilderAttributes;
}

impl<T> EngineTypes for EthEngineTypes<T>
where
    T: PayloadTypes,
    T::BuiltPayload: BuiltPayload<Primitives: NodePrimitives<Block = reth_primitives::Block>>
        + TryInto<ExecutionPayloadV1>
        + TryInto<ExecutionPayloadEnvelopeV2>
        + TryInto<ExecutionPayloadEnvelopeV3>
        + TryInto<ExecutionPayloadEnvelopeV4>,
{
    type ExecutionData = ExecutionData;
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = ExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = ExecutionPayloadEnvelopeV4;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> ExecutionData {
        let (payload, sidecar) =
            ExecutionPayload::from_block_unchecked(block.hash(), &block.into_block());
        ExecutionData { payload, sidecar }
    }
}

/// A default payload type for [`EthEngineTypes`]
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct EthPayloadTypes;

impl PayloadTypes for EthPayloadTypes {
    type BuiltPayload = EthBuiltPayload;
    type PayloadAttributes = EthPayloadAttributes;
    type PayloadBuilderAttributes = EthPayloadBuilderAttributes;
}

/// Validator for the ethereum engine API.
#[derive(Debug, Clone)]
pub struct EthereumEngineValidator {
    inner: ExecutionPayloadValidator<ChainSpec>,
}

impl EthereumEngineValidator {
    /// Instantiates a new validator.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { inner: ExecutionPayloadValidator::new(chain_spec) }
    }

    /// Returns the chain spec used by the validator.
    #[inline]
    fn chain_spec(&self) -> &ChainSpec {
        self.inner.chain_spec()
    }
}

impl PayloadValidator for EthereumEngineValidator {
    type Block = Block;
    type ExecutionData = ExecutionData;

    fn ensure_well_formed_payload(
        &self,
        payload: ExecutionData,
    ) -> Result<SealedBlock, PayloadError> {
        self.inner.ensure_well_formed_payload(payload)
    }
}

impl<Types> EngineValidator<Types> for EthereumEngineValidator
where
    Types: EngineTypes<PayloadAttributes = EthPayloadAttributes, ExecutionData = ExecutionData>,
{
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, EthPayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(self.chain_spec(), version, payload_or_attrs)
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &EthPayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(self.chain_spec(), version, attributes.into())
    }
}
