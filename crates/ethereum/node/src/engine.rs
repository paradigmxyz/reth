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
    validate_execution_requests, validate_payload_timestamp, validate_version_specific_fields,
    EngineApiMessageVersion, EngineObjectValidationError, MessageValidationKind, NewPayloadError,
    PayloadOrAttributes, VersionSpecificValidationError,
};
use reth_primitives_traits::SealedBlock;
use std::sync::Arc;

/// Validator for the ethereum engine API.
#[derive(Debug, Clone)]
pub struct EthereumEngineValidator<ChainSpec = reth_chainspec::ChainSpec> {
    inner: EthereumExecutionPayloadValidator<ChainSpec>,
    proofs: reth_provider::DependencyProofStore,
}

impl<ChainSpec> EthereumEngineValidator<ChainSpec> {
    /// Instantiates a new validator.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self {
            inner: EthereumExecutionPayloadValidator::new(chain_spec),
            proofs: reth_provider::DependencyProofStore::unconfigured(),
        }
    }

    /// Shares the durable proof store used by consensus validation and RPC retrieval.
    pub fn with_dependency_proofs(mut self, proofs: reth_provider::DependencyProofStore) -> Self {
        self.proofs = proofs;
        self
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

    fn validates_block_supplemental_data(&self) -> bool {
        true
    }

    fn convert_payload_to_block(
        &self,
        payload: ExecutionData,
    ) -> Result<Arc<SealedBlock<Self::Block>>, NewPayloadError> {
        let proof = payload.payload.as_v5().map(|payload| payload.dependency_proof.clone());
        let block = self.inner.ensure_well_formed_payload(payload)?;
        if self.chain_spec().is_bogota_active_at_timestamp(block.timestamp) {
            let proof = proof.ok_or_else(|| {
                NewPayloadError::other(reth_errors::ConsensusError::MissingDependencyProof)
            })?;
            let dependencies = alloy_consensus::TxEip8141::collect_signature_dependencies(
                &block.body().transactions,
            )
            .map_err(NewPayloadError::other)?;
            alloy_eips::eip8288::verify_proof(&dependencies, &proof).map_err(|err| {
                if err == alloy_eips::eip8288::ProofError::VerifierUnavailable {
                    NewPayloadError::Unavailable(Box::new(err))
                } else {
                    NewPayloadError::other(err)
                }
            })?;
            self.proofs
                .insert(block.hash(), proof)
                .map_err(|err| NewPayloadError::Unavailable(Box::new(err)))?;
        }
        Ok(Arc::new(block))
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
        if version == EngineApiMessageVersion::V6 {
            for (present, error) in [
                (
                    attributes.withdrawals.is_some(),
                    VersionSpecificValidationError::NoWithdrawalsPostShanghai,
                ),
                (
                    attributes.parent_beacon_block_root.is_some(),
                    VersionSpecificValidationError::NoParentBeaconBlockRootPostCancun,
                ),
                (
                    attributes.slot_number.is_some(),
                    VersionSpecificValidationError::NoSlotNumberPostAmsterdam,
                ),
            ] {
                if !present {
                    return Err(EngineObjectValidationError::PayloadAttributes(error));
                }
            }
            if attributes.target_gas_limit.is_none() {
                return Err(EngineObjectValidationError::PayloadAttributes(
                    VersionSpecificValidationError::MissingTargetGasLimit,
                ));
            }
            let lists = attributes.inclusion_lists.as_ref().ok_or(
                EngineObjectValidationError::PayloadAttributes(
                    VersionSpecificValidationError::MissingInclusionLists,
                ),
            )?;
            if !lists.is_empty() {
                return Err(EngineObjectValidationError::PayloadAttributes(
                    VersionSpecificValidationError::UnsupportedInclusionLists,
                ));
            }
        } else if attributes.inclusion_lists.is_some() {
            return Err(EngineObjectValidationError::PayloadAttributes(
                VersionSpecificValidationError::UnsupportedInclusionLists,
            ));
        }
        if version == EngineApiMessageVersion::V6 {
            validate_payload_timestamp(
                self.chain_spec(),
                version,
                attributes.timestamp,
                MessageValidationKind::PayloadAttributes,
            )?;
        }
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
    use alloy_consensus::{proofs::calculate_withdrawals_root, BlockBody, Header};
    use alloy_eips::{
        eip7685::EMPTY_REQUESTS_HASH,
        eip8288::{dependency_digest, ProofError},
    };
    use alloy_primitives::{keccak256, Bytes, U256};
    use alloy_rpc_types_engine::PayloadStatusEnum;
    use reth_chainspec::ChainSpecBuilder;
    use reth_ethereum_engine_primitives::{EthBuiltPayload, EthEngineTypes};
    use reth_primitives_traits::RecoveredBlock;

    fn empty_dependency_payload(proof: Bytes) -> ExecutionData {
        let bal = Bytes::from_static(&[0xc0]);
        let block = Block {
            header: Header {
                timestamp: 1,
                gas_limit: 30_000_000,
                base_fee_per_gas: Some(1),
                withdrawals_root: Some(calculate_withdrawals_root(&[])),
                parent_beacon_block_root: Some(Default::default()),
                requests_hash: Some(EMPTY_REQUESTS_HASH),
                blob_gas_used: Some(0),
                excess_blob_gas: Some(0),
                block_access_list_hash: Some(keccak256(&bal)),
                slot_number: Some(1),
                dependency_proof_hash: Some(keccak256(&proof)),
                block_deps_hash: Some(dependency_digest(&[])),
                ..Default::default()
            },
            body: BlockBody { withdrawals: Some(Default::default()), ..Default::default() },
        };
        EthBuiltPayload::new(
            Arc::new(RecoveredBlock::new_unhashed(block, vec![])),
            U256::ZERO,
            Some(Default::default()),
            Some(bal),
        )
        .with_dependency_proof(Some(proof))
        .into_execution_data()
    }

    #[test]
    fn unavailable_storage_does_not_invalidate_a_payload() {
        let validator = EthereumEngineValidator::new(Arc::new(
            ChainSpecBuilder::mainnet().bogota_activated().build(),
        ));
        let error = PayloadValidator::<EthEngineTypes>::convert_payload_to_block(
            &validator,
            empty_dependency_payload(Bytes::new()),
        )
        .unwrap_err();
        assert!(matches!(error, NewPayloadError::Unavailable(_)));
        assert_eq!(PayloadStatusEnum::from(error), PayloadStatusEnum::Syncing);
    }

    #[test]
    fn nonempty_proof_for_empty_dependencies_is_invalid_before_storage() {
        let validator = EthereumEngineValidator::new(Arc::new(
            ChainSpecBuilder::mainnet().bogota_activated().build(),
        ));
        let error = PayloadValidator::<EthEngineTypes>::convert_payload_to_block(
            &validator,
            empty_dependency_payload(Bytes::from_static(&[1])),
        )
        .unwrap_err();
        assert!(
            matches!(&error, NewPayloadError::Other(error) if error.downcast_ref::<ProofError>() == Some(&ProofError::Proof))
        );
        assert!(PayloadStatusEnum::from(error).is_invalid());
    }
    #[test]
    fn v6_attributes_check_required_fields_before_fork_timeframe() {
        let validator = EthereumEngineValidator::new(reth_chainspec::MAINNET.clone());
        let mut attrs: EthPayloadAttributes =
            alloy_rpc_types_engine::PayloadAttributesV6::default().into();
        assert!(matches!(
            EngineApiValidator::<EthEngineTypes>::ensure_well_formed_attributes(
                &validator,
                EngineApiMessageVersion::V6,
                &attrs
            ),
            Err(EngineObjectValidationError::UnsupportedFork)
        ));
        attrs.withdrawals = None;
        assert!(matches!(
            EngineApiValidator::<EthEngineTypes>::ensure_well_formed_attributes(
                &validator,
                EngineApiMessageVersion::V6,
                &attrs
            ),
            Err(EngineObjectValidationError::PayloadAttributes(
                VersionSpecificValidationError::NoWithdrawalsPostShanghai
            ))
        ));
    }
}
