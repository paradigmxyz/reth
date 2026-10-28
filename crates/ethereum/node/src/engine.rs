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
        let block_access_list = payload.payload.block_access_list().cloned();
        self.inner.ensure_well_formed_payload(payload).map_err(|error| {
            // Hashing malformed BAL bytes can disagree with a header committed to the canonical
            // encoding. Preserve the decoding error instead of masking it as a hash mismatch.
            if error.is_block_hash_mismatch() &&
                let Some(raw) = block_access_list &&
                let Err(error) = alloy_eips::eip7928::bal::DecodedBal::from_rlp_bytes(raw)
            {
                return NewPayloadError::BlockAccessListDecode(error)
            }
            error.into()
        })
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
    use alloy_consensus::Header;
    use alloy_eips::eip7928::{AccountChanges, BlockAccessIndex, NonceChange};
    use alloy_primitives::{Address, Bytes, B256};
    use alloy_rpc_types_engine::{ExecutionPayload, ExecutionPayloadSidecar, ExecutionPayloadV4};
    use reth_ethereum_engine_primitives::EthEngineTypes;

    fn payload_with_nonce_bump() -> ExecutionData {
        let account = AccountChanges {
            address: Address::repeat_byte(0x11),
            nonce_changes: vec![NonceChange::new(BlockAccessIndex::PRE_EXECUTION, 1)],
            ..Default::default()
        };
        let raw = Bytes::from(alloy_rlp::encode(vec![account]));
        let block = Block {
            header: Header { base_fee_per_gas: Some(1), ..Default::default() },
            body: Default::default(),
        };
        let payload = ExecutionPayloadV4::from_block_unchecked_with_bal(B256::ZERO, &block, raw);
        let sidecar = ExecutionPayloadSidecar::none();
        let mut payload = ExecutionPayload::V4(payload);
        let hash = payload
            .clone()
            .try_into_block_with_sidecar::<reth_ethereum_primitives::TransactionSigned>(&sidecar)
            .unwrap()
            .seal_slow()
            .hash();
        if let ExecutionPayload::V4(payload) = &mut payload {
            payload.payload_inner.payload_inner.payload_inner.block_hash = hash;
        }
        ExecutionData { payload, sidecar }
    }

    fn convert(payload: ExecutionData) -> Result<SealedBlock<Block>, NewPayloadError> {
        let validator = EthereumEngineValidator::new(reth_chainspec::MAINNET.clone());
        <EthereumEngineValidator as PayloadValidator<EthEngineTypes>>::convert_payload_to_block(
            &validator, payload,
        )
    }

    #[test]
    fn malformed_bal_precedes_block_hash_mismatch() {
        let mut data = payload_with_nonce_bump();
        let ExecutionPayload::V4(payload) = &mut data.payload else { unreachable!() };
        let mut raw = payload.block_access_list.to_vec();
        let position = raw.windows(4).position(|bytes| bytes == [0xc3, 0xc2, 0x80, 1]).unwrap();
        raw[position + 2] = 0;
        payload.block_access_list = Bytes::from(raw);
        // The advertised header still commits to the canonical encoding of index zero.
        assert!(matches!(
            convert(data),
            Err(NewPayloadError::BlockAccessListDecode(alloy_rlp::Error::LeadingZero))
        ));
    }

    #[test]
    fn canonical_bal_preserves_block_hash_mismatch() {
        let mut data = payload_with_nonce_bump();
        let ExecutionPayload::V4(payload) = &mut data.payload else { unreachable!() };
        payload.payload_inner.payload_inner.payload_inner.block_hash = B256::ZERO;
        assert!(convert(data).unwrap_err().is_block_hash_mismatch());
    }
}
