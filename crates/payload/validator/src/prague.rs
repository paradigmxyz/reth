//! Prague rules for new payloads.

use alloy_rpc_types::engine::MaybePraguePayloadFields;
use reth_primitives_traits::BlockBody;

/// Checks block and sidecar well-formedness w.r.t new Prague fields and new transaction type
/// EIP-7702.
///
/// Checks that:
/// - Prague fields are present if Prague is active and vv
/// - does not contain EIP-7702 transactions if Prague is not active
#[inline]
pub fn ensure_well_formed_fields<T: BlockBody>(
    block_body: T,
    prague_fields: MaybePraguePayloadFields,
    is_prague_active: bool,
) {
    ensure_well_formed_sidecar_fields(prague_fields, is_prague_active)?;
    ensure_well_formed_transactions_field(block_body, is_prague_active)
}

/// Checks that Prague fields are present on sidecar if Prague is active and vv.
#[inline]
pub fn ensure_well_formed_sidecar_fields<T: Block>(
    prague_fields: MaybePraguePayloadFields,
    is_prague_active: bool,
) -> Result<(), PayloadError> {
    if is_prague_active {
        if prague_fields.is_none() {
            // prague active but prague fields not present
            return Err(PayloadError::PostPragueWithoutPragueFields)
        }
    } else {
        if prague_fields.is_some() {
            // prague _not_ active but prague fields present
            return Err(PayloadError::PrePragueWithPragueFields)
        }
    }

    Ok(())
}

/// Checks that transactions field doesn't contain EIP-7702 transactions if Prague is not
/// active.
#[inline]
pub fn ensure_well_formed_transactions_field<T: BlockBody>(
    block_body: T,
    is_prague_active: bool,
) -> Result<(), PayloadError> {
    if !is_prague_active && block_body.has_eip7702_transactions() {
        return Err(PayloadError::PrePragueBlockWithEip7702Transactions)
    }

    Ok(())
}
