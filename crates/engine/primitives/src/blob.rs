//! Engine API blob response types.

use alloc::vec::Vec;
use alloy_eips::{eip4844::Bytes48, eip7594::Cell};

/// Blob cells type returned in responses to `engine_getBlobsV4`:
/// <https://github.com/ethereum/execution-apis/pull/774>
///
/// TODO: Replace this with `alloy_eips::eip4844::BlobCellsAndProofsV1` once the
/// type is available in the Alloy version used by Reth.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlobCellsAndProofsV1 {
    /// The requested blob cells.
    pub blob_cells: Vec<Option<Cell>>,
    /// The KZG proofs for the requested blob cells.
    pub proofs: Vec<Option<Bytes48>>,
}
