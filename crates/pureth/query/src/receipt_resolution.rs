use crate::{receipt_log_address_gindex, BoundsError, GindexError, ResolvedPath};
use alloy_primitives::Address;
pub use reth_pureth_receipt::ReceiptsSsz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptResolutionError {
    Bounds(BoundsError),
    Gindex(GindexError),
}

pub fn resolve_receipt_log_address(
    receipts: &ReceiptsSsz,
    path: ResolvedPath,
) -> Result<(Address, u64), ReceiptResolutionError> {
    let ResolvedPath::ReceiptLogAddress { receipt_index, log_index } = path;
    let receipt_index = usize::try_from(receipt_index)
        .map_err(|_| ReceiptResolutionError::Bounds(BoundsError::IndexTooLarge))?;
    let log_index = usize::try_from(log_index)
        .map_err(|_| ReceiptResolutionError::Bounds(BoundsError::IndexTooLarge))?;
    let receipt = receipts
        .get(receipt_index)
        .ok_or(ReceiptResolutionError::Bounds(BoundsError::ReceiptOutOfBounds))?;
    let log = receipt
        .logs()
        .get(log_index)
        .ok_or(ReceiptResolutionError::Bounds(BoundsError::LogOutOfBounds))?;
    let gindex = receipt_log_address_gindex(path).map_err(ReceiptResolutionError::Gindex)?;

    Ok((log.address(), gindex))
}
