//! Validates execution payload wrt Ethereum consensus rules

use alloy_consensus::Block;
use alloy_rpc_types_engine::{ExecutionData, PayloadError};
use reth_chainspec::EthereumHardforks;
use reth_payload_validator::{cancun, prague, shanghai};
use reth_primitives_traits::{Block as _, SealedBlock, SignedTransaction};
use std::sync::Arc;

/// Execution payload validator.
#[derive(Clone, Debug)]
pub struct EthereumExecutionPayloadValidator<ChainSpec> {
    /// Chain spec to validate against.
    chain_spec: Arc<ChainSpec>,
    /// Whether to allow pre-Amsterdam BAL payloads.
    allow_pre_amsterdam_bal: bool,
}

impl<ChainSpec> EthereumExecutionPayloadValidator<ChainSpec> {
    /// Create a new validator.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec, allow_pre_amsterdam_bal: false }
    }

    /// Allow pre-Amsterdam BAL payloads.
    pub const fn with_allow_pre_amsterdam_bal(mut self, allow: bool) -> Self {
        self.allow_pre_amsterdam_bal = allow;
        self
    }

    /// Returns the chain spec used by the validator.
    #[inline]
    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        &self.chain_spec
    }
}

impl<ChainSpec: EthereumHardforks> EthereumExecutionPayloadValidator<ChainSpec> {
    /// Ensures that the given payload does not violate any consensus rules that concern the block's
    /// layout,
    ///
    /// See also [`ensure_well_formed_payload`]
    pub fn ensure_well_formed_payload<T: SignedTransaction>(
        &self,
        payload: ExecutionData,
    ) -> Result<SealedBlock<Block<T>>, PayloadError> {
        ensure_well_formed_payload_inner(&self.chain_spec, payload, self.allow_pre_amsterdam_bal)
    }
}

/// Ensures that the given payload does not violate any consensus rules that concern the block's
/// layout, like:
///    - missing or invalid base fee
///    - invalid extra data
///    - invalid transactions
///    - incorrect hash
///    - the versioned hashes passed with the payload do not exactly match transaction versioned
///      hashes
///    - the block does not contain blob transactions if it is pre-cancun
///
/// The checks are done in the order that conforms with the engine-API specification.
///
/// This is intended to be invoked after receiving the payload from the CLI.
/// The additional [`MaybeCancunPayloadFields`](alloy_rpc_types_engine::MaybeCancunPayloadFields) are not part of the payload, but are additional fields in the `engine_newPayloadV3` RPC call, See also <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#engine_newpayloadv3>
///
/// If the cancun fields are provided this also validates that the versioned hashes in the block
/// match the versioned hashes passed in the
/// [`CancunPayloadFields`](alloy_rpc_types_engine::CancunPayloadFields), if the cancun payload
/// fields are provided. If the payload fields are not provided, but versioned hashes exist
/// in the block, this is considered an error: [`PayloadError::InvalidVersionedHashes`].
///
/// This validates versioned hashes according to the Engine API Cancun spec:
/// <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#specification>
pub fn ensure_well_formed_payload<ChainSpec, T>(
    chain_spec: ChainSpec,
    payload: ExecutionData,
) -> Result<SealedBlock<Block<T>>, PayloadError>
where
    ChainSpec: EthereumHardforks,
    T: SignedTransaction,
{
    ensure_well_formed_payload_inner(chain_spec, payload, false)
}

fn ensure_well_formed_payload_inner<ChainSpec, T>(
    chain_spec: ChainSpec,
    payload: ExecutionData,
    allow_pre_amsterdam_bal: bool,
) -> Result<SealedBlock<Block<T>>, PayloadError>
where
    ChainSpec: EthereumHardforks,
    T: SignedTransaction,
{
    let ExecutionData { payload, sidecar } = payload;

    let expected_hash = payload.block_hash();
    let should_ignore_pre_amsterdam_bal = allow_pre_amsterdam_bal &&
        payload.block_access_list().is_some() &&
        !chain_spec.is_amsterdam_active_at_timestamp(payload.timestamp());

    let mut block = payload.try_into_block_with_sidecar(&sidecar)?;

    if should_ignore_pre_amsterdam_bal {
        block.header.block_access_list_hash = None;
        block.header.slot_number = None;
    }

    let sealed_block = block.seal_slow();

    if expected_hash != sealed_block.hash() {
        return Err(PayloadError::BlockHash {
            execution: sealed_block.hash(),
            consensus: expected_hash,
        })
    }

    shanghai::ensure_well_formed_fields(
        sealed_block.body(),
        chain_spec.is_shanghai_active_at_timestamp(sealed_block.timestamp),
    )?;

    cancun::ensure_well_formed_fields(
        &sealed_block,
        sidecar.cancun(),
        chain_spec.is_cancun_active_at_timestamp(sealed_block.timestamp),
    )?;

    prague::ensure_well_formed_fields(
        sealed_block.body(),
        sidecar.prague(),
        chain_spec.is_prague_active_at_timestamp(sealed_block.timestamp),
    )?;

    Ok(sealed_block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{EMPTY_OMMER_ROOT_HASH, EMPTY_ROOT_HASH};
    use alloy_eips::eip7685::EMPTY_REQUESTS_HASH;
    use alloy_primitives::{Bytes, B256};
    use alloy_rpc_types_engine::{ExecutionPayload, ExecutionPayloadSidecar, ExecutionPayloadV4};
    use reth_chainspec::ChainSpecBuilder;
    use reth_ethereum_primitives::{Block as EthBlock, TransactionSigned};

    #[test]
    fn pre_amsterdam_bal_payload_is_opt_in() {
        let block = empty_prague_block();
        let original_hash = block.clone().seal_slow().hash();
        let raw_bal = Bytes::from_static(&[0xc0]);
        let payload = ExecutionPayload::V4(ExecutionPayloadV4::from_block_unchecked_with_bal(
            original_hash,
            &block,
            raw_bal,
        ));
        let execution_data =
            ExecutionData { payload, sidecar: ExecutionPayloadSidecar::from_block(&block) };

        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().prague_activated().build());
        assert!(matches!(
            EthereumExecutionPayloadValidator::new(chain_spec.clone())
                .ensure_well_formed_payload::<TransactionSigned>(execution_data.clone())
                .unwrap_err(),
            PayloadError::BlockHash { .. }
        ));

        let sealed = EthereumExecutionPayloadValidator::new(chain_spec)
            .with_allow_pre_amsterdam_bal(true)
            .ensure_well_formed_payload::<TransactionSigned>(execution_data)
            .unwrap();

        assert_eq!(sealed.hash(), original_hash);
        assert!(sealed.header().block_access_list_hash.is_none());
        assert!(sealed.header().slot_number.is_none());
    }

    fn empty_prague_block() -> EthBlock {
        let mut block = EthBlock::default();
        block.header.ommers_hash = EMPTY_OMMER_ROOT_HASH;
        block.header.transactions_root = EMPTY_ROOT_HASH;
        block.header.receipts_root = EMPTY_ROOT_HASH;
        block.header.withdrawals_root = Some(EMPTY_ROOT_HASH);
        block.header.base_fee_per_gas = Some(0);
        block.header.parent_beacon_block_root = Some(B256::ZERO);
        block.header.blob_gas_used = Some(0);
        block.header.excess_blob_gas = Some(0);
        block.header.requests_hash = Some(EMPTY_REQUESTS_HASH);
        block.body.withdrawals = Some(Default::default());
        block
    }
}
