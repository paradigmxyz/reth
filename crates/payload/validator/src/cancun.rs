//! Cancun rules for new payloads.

use alloy_rpc_types::engine::MaybeCancunPayloadFields;
use reth_primitives_traits::{Block, BlockBody};

/// Checks block and sidecar w.r.t new Cancun fields and new transaction type EIP-4844.
///
/// Checks that:
/// - Cancun fields are present if Cancun is active and vv
/// - contains EIP-4844 transactions if Cancun is active and vv
/// - checks blob versioned hashes in block and sidecar match
#[inline]
pub fn ensure_well_formed_fields<T: Block>(
    block: &SealedBlock<T>,
    cancun_fields: MaybeCancunPayloadFields,
    is_cancun_active: bool,
) -> Result<(), PayloadError> {
    ensure_well_formed_header_and_sidecar_fields(block, cancun_fields, is_cancun_active)?;
    ensure_well_formed_transactions_field(block.body(), is_cancun_active)?;
    if is_cancun_active {
        ensure_matching_blob_versioned_hashes(block, cancun_fields)?;
    }
}

/// Checks that Cancun fields on block header and sidecar are present if Cancun is active and vv.
#[inline]
pub fn ensure_well_formed_header_and_sidecar_fields<T: Block>(
    block: &SealedBlock<T>,
    cancun_fields: &MaybeCancunPayloadFields,
    is_cancun_active: bool,
) -> Result<(), PayloadError> {
    if is_cancun_active {
        if block.blob_gas_used().is_none() {
            // cancun active but blob gas used not present
            return Err(PayloadError::PostCancunBlockWithoutBlobGasUsed)
        }
        if block.excess_blob_gas.is_none() {
            // cancun active but excess blob gas not present
            return Err(PayloadError::PostCancunBlockWithoutExcessBlobGas)
        }
        if sidecar.cancun().is_none() {
            // cancun active but cancun fields not present
            return Err(PayloadError::PostCancunWithoutCancunFields)
        }
    } else {
        if block.blob_gas_used().is_some() {
            // cancun not active but blob gas used present
            return Err(PayloadError::PreCancunBlockWithBlobGasUsed)
        }
        if block.excess_blob_gas().is_some() {
            // cancun not active but excess blob gas present
            return Err(PayloadError::PreCancunBlockWithExcessBlobGas)
        }
        if sidecar.cancun().is_some() {
            // cancun not active but cancun fields present
            return Err(PayloadError::PreCancunWithCancunFields)
        }
    }
}

/// Checks transactions field and sidecar w.r.t new Cancun fields and new transaction type EIP-4844.
///
/// Checks that:
/// - contains EIP-4844 transactions if Cancun is active and vv
/// - checks blob versioned hashes in block and sidecar match
#[inline]
pub fn ensure_well_formed_transactions_field_with_sidecar<T: BlockBody>(
    block_body: T,
    cancun_fields: &MaybeCancunPayloadFields,
    is_cancun_active: bool,
) -> Result<(), PayloadError> {
    if is_cancun_active {
        if !block_body.has_eip4844_transactions() {
            return Err(PayloadError::PostCancunBlockWithoutBlobTransactions)
        }
        ensure_matching_blob_versioned_hashes(block_body, cancun_fields)
    } else {
        if block_body.has_eip4844_transactions() {
            return Err(PayloadError::PreCancunBlockWithBlobTransactions)
        }
    }

    Ok(())
}

/// Ensures that the number of blob versioned hashes of a EIP-4844 transactions in block, matches
/// the number hashes included in the _separate_ `block_versioned_hashes` of the cancun payload
/// fields on the sidecar.
pub fn ensure_matching_blob_versioned_hashes<T: BlockBody>(
    block_body: T,
    cancun_fields: &MaybeCancunPayloadFields,
) -> Result<(), PayloadError> {
    let num_blob_versioned_hashes = block_body.blob_versioned_hashes_iter().count();
    // Additional Cancun checks for blob transactions
    if let Some(versioned_hashes) = cancun_fields.versioned_hashes() {
        if num_blob_versioned_hashes != versioned_hashes.len() {
            // Number of blob versioned hashes does not match
            return Err(PayloadError::InvalidVersionedHashes)
        }
        // we can use `zip` safely here because we already compared their length
        for (payload_versioned_hash, block_versioned_hash) in
            versioned_hashes.iter().zip(block_body.blob_versioned_hashes_iter())
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
