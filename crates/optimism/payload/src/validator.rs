//! Validates execution payload wrt Optimism consensus rules

use alloc::sync::Arc;
use alloy_rpc_types_engine::{ExecutionPayload, ExecutionPayloadSidecar, PayloadError};
use derive_more::Deref;
use op_alloy_rpc_types_engine::OpExecutionData;
use reth_optimism_forks::OpHardforks;
use reth_payload_validator::ExecutionPayloadValidator;
use reth_primitives::{Block, SealedBlock};
use reth_primitives_traits::{Block as _, BlockBody, SignedTransaction};

/// Execution payload validator.
#[derive(Clone, Debug, Deref)]
pub struct OpExecutionPayloadValidator<ChainSpec> {
    /// Chain spec to validate against.
    #[deref]
    inner: ExecutionPayloadValidator<ChainSpec>,
}

impl<ChainSpec> OpExecutionPayloadValidator<ChainSpec> {
    /// Returns a new instance.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { inner: ExecutionPayloadValidator::new(chain_spec) }
    }
}

impl<ChainSpec> OpExecutionPayloadValidator<ChainSpec>
where
    ChainSpec: OpHardforks,
{
    /// Ensures that the given payload does not violate any consensus rules that concern the block's
    /// layout, like:
    ///    - missing or invalid base fee
    ///    - invalid extra data
    ///    - invalid transactions
    ///    - incorrect hash
    ///    - block contains blob transactions or blob versioned hashes
    ///    - block contains l1 withdrawals
    ///
    /// The checks are done in the order that conforms with the engine-API specification.
    ///
    /// This is intended to be invoked after receiving the payload from the CLI.
    /// The additional fields, starting with [`MaybeCancunPayloadFields`](alloy_rpc_types::engine::MaybeCancunPayloadFields), are not part of the payload, but are additional fields starting in the `engine_newPayloadV3` RPC call, See also <https://specs.optimism.io/protocol/exec-engine.html#engine_newpayloadv3>
    ///
    /// If the cancun fields are provided this also validates that the versioned hashes in the block
    /// are empty as well as those passed in the sidecar. If the payload fields are not provided.
    ///
    /// Validation according to specs <https://specs.optimism.io/protocol/exec-engine.html#engine-api>.
    pub fn ensure_well_formed_payload<T: SignedTransaction>(
        &self,
        payload: OpExecutionData,
    ) -> Result<SealedBlock<Block<T>>, PayloadError> {
        let OpExecutionData { payload, sidecar } = payload;

        let expected_hash = payload.block_hash();

        // First parse the block
        let mut block = payload.try_into_block_with_sidecar(&sidecar)?;

        // update withdrawals root to l2 withdrawals root if isthmus active
        if self.chain_spec().is_isthmus_active_at_timestamp(block.timestamp) {
            // overwrite l1 withdrawals root with l2 withdrawals root, ie with storage root of
            // predeploy `L2ToL1MessagePasser.sol` at `0x42..16`
            match payload.withdrawals_root() {
                Some(l2_withdrawals_root) => block.withdrawals_root = l2_withdrawals_root,
                None => return Err(PayloadError::PreCancunBlockWithBlobTransactions), /* todo: return op specific error */
            }
        }

        let sealed_block = block.seal_slow();

        // Ensure the hash included in the payload matches the block hash
        if expected_hash != sealed_block.hash() {
            return Err(PayloadError::BlockHash {
                execution: sealed_block.hash(),
                consensus: expected_hash,
            })
        }

        if sealed_block.body().has_eip4844_transactions() {
            // todo: needs new error type OpPayloadError
            return Err(PayloadError::PreCancunBlockWithBlobTransactions)
        }

        if self.chain_spec().is_cancun_active_at_timestamp(sealed_block.timestamp) {
            if sealed_block.blob_gas_used.is_none() {
                // cancun active but blob gas used not present
                return Err(PayloadError::PostCancunBlockWithoutBlobGasUsed)
            }
            if sealed_block.excess_blob_gas.is_none() {
                // cancun active but excess blob gas not present
                return Err(PayloadError::PostCancunBlockWithoutExcessBlobGas)
            }
            match sidecar.canyon() {
                None => {
                    // cancun active but cancun fields not present
                    return Err(PayloadError::PostCancunWithoutCancunFields)
                }
                Some(fields) => {
                    if !fields.versioned_hashes.is_empty() {
                        // todo: needs new error type OpPayloadError
                        return Err(PayloadError::PreCancunBlockWithBlobTransactions)
                    }
                }
            }
            match sealed_block.blob_versioned_hashes() {
                None => {
                    // cancun active but blob versioned hashes not present
                    return Err(PayloadError::PostCancunBlockWithoutBlobVersionedHashes)
                }
                Some(hashes) => {
                    if !hashes.is_empty() {
                        // todo: needs new error type OpPayloadError
                        return Err(PayloadError::PreCancunBlockWithBlobTransactions)
                    }
                }
            }
        } else {
            if sealed_block.blob_gas_used.is_some() {
                // cancun not active but blob gas used present
                return Err(PayloadError::PreCancunBlockWithBlobGasUsed)
            }
            if sealed_block.excess_blob_gas.is_some() {
                // cancun not active but excess blob gas present
                return Err(PayloadError::PreCancunBlockWithExcessBlobGas)
            }
            if sidecar.canyon().is_some() {
                // cancun not active but cancun fields present
                return Err(PayloadError::PreCancunWithCancunFields)
            }
        }

        let shanghai_active =
            self.chain_spec().is_shanghai_active_at_timestamp(sealed_block.timestamp);
        if let Some(l1_withdrawals) = sealed_block.body().withdrawals() {
            if shanghai_active {
                if !l1_withdrawals.is_empty() {
                    // todo: needs op specific error
                    return Err(PayloadError::PostShanghaiBlockWithoutWithdrawals)
                }
            } else {
                // shanghai active but withdrawals not present
                return Err(PayloadError::PostShanghaiBlockWithoutWithdrawals)
            }
        }

        if !self.chain_spec().is_prague_active_at_timestamp(sealed_block.timestamp) &&
            sealed_block.body().has_eip7702_transactions()
        {
            return Err(PayloadError::PrePragueBlockWithEip7702Transactions)
        }

        Ok(sealed_block)
    }
}

// todo: needs to return op specific error to be able to unit test
