//! Payload Validation support.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use alloy_rpc_types::engine::{
    ExecutionPayload, ExecutionPayloadSidecar, MaybeCancunPayloadFields, PayloadError,
};
use reth_chainspec::EthereumHardforks;
use reth_primitives::{BlockBody, BlockExt, Header, SealedBlock};
use reth_primitives_traits::SignedTransaction;
use reth_rpc_types_compat::engine::payload::try_into_block;
use std::sync::Arc;

/// Execution payload validator.
#[derive(Clone, Debug)]
pub struct ExecutionPayloadValidator<ChainSpec> {
    /// Chain spec to validate against.
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> ExecutionPayloadValidator<ChainSpec> {
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

impl<ChainSpec: EthereumHardforks> ExecutionPayloadValidator<ChainSpec> {
    /// Returns true if the Cancun hardfork is active at the given timestamp.
    #[inline]
    fn is_cancun_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.chain_spec().is_cancun_active_at_timestamp(timestamp)
    }

    /// Returns true if the Shanghai hardfork is active at the given timestamp.
    #[inline]
    fn is_shanghai_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.chain_spec().is_shanghai_active_at_timestamp(timestamp)
    }

    /// Returns true if the Prague harkdfork is active at the given timestamp.
    #[inline]
    fn is_prague_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.chain_spec().is_prague_active_at_timestamp(timestamp)
    }

    /// Cancun specific checks for EIP-4844 blob transactions.
    ///
    /// Ensures that the number of blob versioned hashes matches the number hashes included in the
    /// _separate_ `block_versioned_hashes` of the cancun payload fields.
    fn ensure_matching_blob_versioned_hashes<T: SignedTransaction>(
        &self,
        sealed_block: &SealedBlock<Header, BlockBody<T>>,
        cancun_fields: &MaybeCancunPayloadFields,
    ) -> Result<(), PayloadError> {
        let num_blob_versioned_hashes = sealed_block.blob_versioned_hashes_iter().count();
        // Additional Cancun checks for blob transactions
        if let Some(versioned_hashes) = cancun_fields.versioned_hashes() {
            if num_blob_versioned_hashes != versioned_hashes.len() {
                // Number of blob versioned hashes does not match
                return Err(PayloadError::InvalidVersionedHashes)
            }
            // we can use `zip` safely here because we already compared their length
            for (payload_versioned_hash, block_versioned_hash) in
                versioned_hashes.iter().zip(sealed_block.blob_versioned_hashes_iter())
            {
                if payload_versioned_hash != block_versioned_hash {
                    return Err(PayloadError::InvalidVersionedHashes)
                }
            }
        } else {
            // No Cancun fields, if block includes any blobs, this is an error
            if num_blob_versioned_hashes > 0 {
                return Err(PayloadError::InvalidVersionedHashes)
            }
        }

        Ok(())
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
    /// The additional [`MaybeCancunPayloadFields`] are not part of the payload, but are additional fields in the `engine_newPayloadV3` RPC call, See also <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#engine_newpayloadv3>
    ///
    /// If the cancun fields are provided this also validates that the versioned hashes in the block
    /// match the versioned hashes passed in the
    /// [`CancunPayloadFields`](alloy_rpc_types::engine::CancunPayloadFields), if the cancun payload
    /// fields are provided. If the payload fields are not provided, but versioned hashes exist
    /// in the block, this is considered an error: [`PayloadError::InvalidVersionedHashes`].
    ///
    /// This validates versioned hashes according to the Engine API Cancun spec:
    /// <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#specification>
    pub fn ensure_well_formed_payload<T: SignedTransaction>(
        &self,
        payload: ExecutionPayload,
        sidecar: ExecutionPayloadSidecar,
    ) -> Result<SealedBlock<Header, BlockBody<T>>, PayloadError> {
        let expected_hash = payload.block_hash();

        // First parse the block
        let sealed_block = try_into_block(payload, &sidecar)?.seal_slow();

        // Ensure the hash included in the payload matches the block hash
        if expected_hash != sealed_block.hash() {
            return Err(PayloadError::BlockHash {
                execution: sealed_block.hash(),
                consensus: expected_hash,
            })
        }

        if self.is_cancun_active_at_timestamp(sealed_block.timestamp) {
            if sealed_block.header.blob_gas_used.is_none() {
                // cancun active but blob gas used not present
                return Err(PayloadError::PostCancunBlockWithoutBlobGasUsed)
            }
            if sealed_block.header.excess_blob_gas.is_none() {
                // cancun active but excess blob gas not present
                return Err(PayloadError::PostCancunBlockWithoutExcessBlobGas)
            }
            if sidecar.cancun().is_none() {
                // cancun active but cancun fields not present
                return Err(PayloadError::PostCancunWithoutCancunFields)
            }
        } else {
            if sealed_block.body.has_eip4844_transactions() {
                // cancun not active but blob transactions present
                return Err(PayloadError::PreCancunBlockWithBlobTransactions)
            }
            if sealed_block.header.blob_gas_used.is_some() {
                // cancun not active but blob gas used present
                return Err(PayloadError::PreCancunBlockWithBlobGasUsed)
            }
            if sealed_block.header.excess_blob_gas.is_some() {
                // cancun not active but excess blob gas present
                return Err(PayloadError::PreCancunBlockWithExcessBlobGas)
            }
            if sidecar.cancun().is_some() {
                // cancun not active but cancun fields present
                return Err(PayloadError::PreCancunWithCancunFields)
            }
        }

        let shanghai_active = self.is_shanghai_active_at_timestamp(sealed_block.timestamp);
        if !shanghai_active && sealed_block.body.withdrawals.is_some() {
            // shanghai not active but withdrawals present
            return Err(PayloadError::PreShanghaiBlockWithWithdrawals)
        }

        if !self.is_prague_active_at_timestamp(sealed_block.timestamp) &&
            sealed_block.body.has_eip7702_transactions()
        {
            return Err(PayloadError::PrePragueBlockWithEip7702Transactions)
        }

        // EIP-4844 checks
        self.ensure_matching_blob_versioned_hashes(
            &sealed_block,
            &sidecar.cancun().cloned().into(),
        )?;

        Ok(sealed_block)
    }
}
