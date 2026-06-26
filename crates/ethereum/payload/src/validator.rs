//! Validates execution payload wrt Ethereum consensus rules

use alloy_consensus::{
    constants::MAXIMUM_EXTRA_DATA_SIZE, proofs::ordered_trie_root_encoded, Block,
};
use alloy_eips::eip2718::Decodable2718;
use alloy_primitives::B256;
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
}

impl<ChainSpec> EthereumExecutionPayloadValidator<ChainSpec> {
    /// Create a new validator.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
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
        ensure_well_formed_payload(&self.chain_spec, payload)
    }

    /// Ensures the payload is well formed and returns the transaction root calculated from the raw
    /// EIP-2718 transaction bytes during conversion.
    pub fn ensure_well_formed_payload_with_tx_root<T: SignedTransaction>(
        &self,
        payload: ExecutionData,
    ) -> Result<(SealedBlock<Block<T>>, B256), PayloadError> {
        ensure_well_formed_payload_with_tx_root(&self.chain_spec, payload)
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
    ensure_well_formed_payload_with_optional_tx_root(chain_spec, payload, false)
        .map(|(block, _)| block)
}

/// Ensures that the given payload is well formed and returns the transaction root calculated from
/// the raw EIP-2718 transaction bytes.
pub fn ensure_well_formed_payload_with_tx_root<ChainSpec, T>(
    chain_spec: ChainSpec,
    payload: ExecutionData,
) -> Result<(SealedBlock<Block<T>>, B256), PayloadError>
where
    ChainSpec: EthereumHardforks,
    T: SignedTransaction,
{
    let (sealed_block, tx_root) =
        ensure_well_formed_payload_with_optional_tx_root(chain_spec, payload, true)?;
    Ok((sealed_block, tx_root.expect("transaction root requested")))
}

fn ensure_well_formed_payload_with_optional_tx_root<ChainSpec, T>(
    chain_spec: ChainSpec,
    payload: ExecutionData,
    reuse_tx_root: bool,
) -> Result<(SealedBlock<Block<T>>, Option<B256>), PayloadError>
where
    ChainSpec: EthereumHardforks,
    T: SignedTransaction,
{
    let ExecutionData { payload, sidecar } = payload;

    if payload.as_v1().extra_data.len() > MAXIMUM_EXTRA_DATA_SIZE {
        return Err(PayloadError::ExtraData(payload.as_v1().extra_data.clone()))
    }

    let expected_hash = payload.block_hash();
    let transactions_root =
        reuse_tx_root.then(|| ordered_trie_root_encoded(payload.transactions()));

    // First parse the block
    let raw_block = if let Some(transactions_root) = transactions_root {
        payload.into_block_with_sidecar_raw_with_transactions_root(&sidecar, transactions_root)?
    } else {
        payload.into_block_with_sidecar_raw(&sidecar)?
    };
    let sealed_block = raw_block.try_map_transactions(decode_transaction)?.seal_slow();

    // Ensure the hash included in the payload matches the block hash
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

    Ok((sealed_block, transactions_root))
}

fn decode_transaction<T: Decodable2718>(tx: alloy_primitives::Bytes) -> Result<T, PayloadError> {
    T::decode_2718_exact(tx.as_ref())
        .map_err(alloy_rlp::Error::from)
        .map_err(PayloadError::from)
}
