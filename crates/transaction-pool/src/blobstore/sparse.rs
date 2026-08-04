//! Reth-owned sparse blob sidecar representation.

use alloy_eips::{
    eip4844::Bytes48,
    eip7594::{Cell, CELLS_PER_EXT_BLOB},
};
use alloy_primitives::B128;

/// A sparse EIP-7594 sidecar.
///
/// The cell vectors are flattened in blob-major, cell-index order. For `n` blobs, the layout is
/// `[blob_0_cell_0..blob_0_cell_127, blob_1_cell_0..]`. Commitments and cell proofs remain
/// present even when blob cells are missing. This lets the networking layer accumulate cells
/// without manufacturing zero-filled blob data that could be mistaken for a real blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SparseBlobSidecar {
    /// One commitment per blob.
    pub commitments: Vec<Bytes48>,
    /// One cell proof per blob and cell, in the same flattened layout as [`Self::cells`].
    pub cell_proofs: Vec<Bytes48>,
    /// Received cells. `None` means that the cell has not been received yet.
    pub cells: Vec<Option<Cell>>,
}

impl SparseBlobSidecar {
    /// Creates an empty sparse sidecar with the given commitments and cell proofs.
    pub fn empty(
        commitments: Vec<Bytes48>,
        cell_proofs: Vec<Bytes48>,
    ) -> Result<Self, SparseBlobSidecarError> {
        let cells = vec![None; expected_cell_count(commitments.len())?];
        Self::try_new(commitments, cell_proofs, cells)
    }

    /// Creates a sparse sidecar from its complete metadata and any received cells.
    pub fn try_new(
        commitments: Vec<Bytes48>,
        cell_proofs: Vec<Bytes48>,
        cells: Vec<Option<Cell>>,
    ) -> Result<Self, SparseBlobSidecarError> {
        let expected = expected_cell_count(commitments.len())?;
        if cell_proofs.len() != expected || cells.len() != expected {
            return Err(SparseBlobSidecarError::InvalidLayout {
                blobs: commitments.len(),
                cell_proofs: cell_proofs.len(),
                cells: cells.len(),
                expected,
            });
        }

        Ok(Self { commitments, cell_proofs, cells })
    }

    /// Returns the number of blobs represented by this sidecar.
    pub const fn blob_count(&self) -> usize {
        self.commitments.len()
    }

    /// Returns the common cell mask available for every blob in this sidecar.
    ///
    /// eth/72 announces one mask for all blob transactions in a message. The intersection is
    /// therefore the safe mask to advertise when different blobs have different received cells.
    pub fn cell_mask(&self) -> B128 {
        if self.commitments.is_empty() {
            return B128::from(0u128)
        }

        let mut mask = u128::MAX;
        for blob_index in 0..self.blob_count() {
            let start = blob_index * CELLS_PER_EXT_BLOB;
            let blob_mask = self.cells[start..start + CELLS_PER_EXT_BLOB]
                .iter()
                .enumerate()
                .fold(0u128, |mask, (cell_index, cell)| {
                    mask | (u128::from(cell.is_some()) << cell_index)
                });
            mask &= blob_mask;
        }
        B128::from(mask)
    }

    /// Returns whether every cell for every blob has been received.
    pub fn is_complete(&self) -> bool {
        !self.commitments.is_empty() && self.cells.iter().all(Option::is_some)
    }

    /// Merges cells selected by `cell_mask` into the sidecar.
    ///
    /// Cells must be ordered by blob first and cell index second, matching the `Cells` response
    /// shape. The operation is all-or-nothing: a malformed count or conflicting cell is rejected
    /// before the sidecar is modified.
    pub fn merge_cells(
        &mut self,
        cell_mask: B128,
        cells: Vec<Cell>,
    ) -> Result<usize, SparseBlobSidecarError> {
        let indices = cell_indices(cell_mask);
        let expected = indices.len().saturating_mul(self.blob_count());
        if cells.len() != expected {
            return Err(SparseBlobSidecarError::CellCountMismatch { expected, actual: cells.len() })
        }

        let mut updates = Vec::with_capacity(expected);
        let mut offset = 0;
        for blob_index in 0..self.blob_count() {
            let start = blob_index * CELLS_PER_EXT_BLOB;
            for &cell_index in &indices {
                let index = start + cell_index;
                updates.push((index, cells[offset]));
                offset += 1;
            }
        }

        for &(index, ref cell) in &updates {
            if let Some(existing) = &self.cells[index] &&
                existing != cell
            {
                return Err(SparseBlobSidecarError::ConflictingCell(index));
            }
        }

        let mut inserted = 0;
        for (index, cell) in updates {
            if self.cells[index].is_none() {
                inserted += 1;
                self.cells[index] = Some(cell);
            }
        }
        Ok(inserted)
    }

    /// Returns the requested cells in blob-major, cell-index order if all are available.
    pub fn cells_for_mask(&self, cell_mask: B128) -> Option<Vec<Cell>> {
        let indices = cell_indices(cell_mask);
        let mut result = Vec::with_capacity(indices.len().saturating_mul(self.blob_count()));
        for blob_index in 0..self.blob_count() {
            let start = blob_index * CELLS_PER_EXT_BLOB;
            for cell_index in &indices {
                result.push(self.cells[start + cell_index]?);
            }
        }
        Some(result)
    }
}

/// Errors produced while constructing or merging sparse sidecars.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SparseBlobSidecarError {
    /// The metadata or cell vectors do not have the required blob-major layout.
    #[error(
        "invalid sparse sidecar layout: {blobs} blobs require {expected} cell proofs and cells, got {cell_proofs} proofs and {cells} cells"
    )]
    InvalidLayout {
        /// Number of commitments/blobs.
        blobs: usize,
        /// Number of supplied cell proofs.
        cell_proofs: usize,
        /// Number of supplied cells.
        cells: usize,
        /// Expected number of proofs and cells.
        expected: usize,
    },
    /// The response did not contain exactly one cell for every requested blob/index pair.
    #[error("invalid sparse cell count: expected {expected}, got {actual}")]
    CellCountMismatch {
        /// Expected number of cells.
        expected: usize,
        /// Actual number of cells.
        actual: usize,
    },
    /// A cell already stored at this index conflicted with a newly received cell.
    #[error("conflicting cell at flattened index {0}")]
    ConflictingCell(usize),
}

fn expected_cell_count(blobs: usize) -> Result<usize, SparseBlobSidecarError> {
    blobs.checked_mul(CELLS_PER_EXT_BLOB).ok_or(SparseBlobSidecarError::InvalidLayout {
        blobs,
        cell_proofs: 0,
        cells: 0,
        expected: usize::MAX,
    })
}

fn cell_indices(mask: B128) -> Vec<usize> {
    let bits = u128::from(mask);
    (0..CELLS_PER_EXT_BLOB).filter(|index| bits & (1u128 << index) != 0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sidecar(blob_count: usize) -> SparseBlobSidecar {
        SparseBlobSidecar::empty(
            vec![Bytes48::default(); blob_count],
            vec![Bytes48::default(); blob_count * CELLS_PER_EXT_BLOB],
        )
        .unwrap()
    }

    #[test]
    fn merges_cells_in_blob_major_order() {
        let mut sidecar = sidecar(2);
        let mask = B128::from((1u128 << 0) | (1u128 << 3));
        let cells = vec![
            Cell::repeat_byte(1),
            Cell::repeat_byte(2),
            Cell::repeat_byte(3),
            Cell::repeat_byte(4),
        ];

        assert_eq!(sidecar.merge_cells(mask, cells).unwrap(), 4);
        assert_eq!(sidecar.cells[0], Some(Cell::repeat_byte(1)));
        assert_eq!(sidecar.cells[3], Some(Cell::repeat_byte(2)));
        assert_eq!(sidecar.cells[CELLS_PER_EXT_BLOB], Some(Cell::repeat_byte(3)));
        assert_eq!(sidecar.cells[CELLS_PER_EXT_BLOB + 3], Some(Cell::repeat_byte(4)));
        assert_eq!(sidecar.cell_mask(), B128::from(0b1001u128));
    }

    #[test]
    fn cell_merge_rejects_conflicts_without_partial_updates() {
        let mut sidecar = sidecar(1);
        let mask = B128::from(1u128);
        sidecar.merge_cells(mask, vec![Cell::repeat_byte(1)]).unwrap();

        let err = sidecar.merge_cells(mask, vec![Cell::repeat_byte(2)]).unwrap_err();
        assert_eq!(err, SparseBlobSidecarError::ConflictingCell(0));
        assert_eq!(sidecar.cells[0], Some(Cell::repeat_byte(1)));
    }

    #[test]
    fn cells_for_mask_requires_all_requested_cells() {
        let mut sidecar = sidecar(1);
        let mask = B128::from(0b11u128);
        assert!(sidecar.cells_for_mask(mask).is_none());

        sidecar.merge_cells(mask, vec![Cell::repeat_byte(1), Cell::repeat_byte(2)]).unwrap();
        assert_eq!(
            sidecar.cells_for_mask(mask),
            Some(vec![Cell::repeat_byte(1), Cell::repeat_byte(2)])
        );
    }
}
