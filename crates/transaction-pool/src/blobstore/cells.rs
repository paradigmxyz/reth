//! Helpers for blob stores that hold
//! [`BlobTransactionCellSidecar`](alloy_eips::eip7594::BlobTransactionCellSidecar)s, which keep
//! only some of each blob's cells.

use alloy_eips::eip4844::BlobCellsAndProofsV1;
use alloy_primitives::{map::B256Set, B256};

// Fills the cells and proofs that `slot` lacks from `found`, since sidecars of different
// transactions may keep different cells of the same blob.
pub(crate) fn merge_cells(slot: &mut Option<BlobCellsAndProofsV1>, found: BlobCellsAndProofsV1) {
    let Some(slot) = slot else {
        *slot = Some(found);
        return
    };
    let cells = slot.blob_cells.iter_mut().zip(&mut slot.proofs);
    for ((cell, proof), (found_cell, found_proof)) in
        cells.zip(found.blob_cells.into_iter().zip(found.proofs))
    {
        if cell.is_none() {
            *cell = found_cell;
            *proof = found_proof;
        }
    }
}

// Returns the requested versioned hashes not found yet.
pub(crate) fn missing_hashes<T>(versioned_hashes: &[B256], result: &[Option<T>]) -> B256Set {
    versioned_hashes
        .iter()
        .zip(result)
        .filter_map(|(hash, found)| found.is_none().then_some(*hash))
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use alloy_eips::{
        eip4844::{env_settings::EnvKzgSettings, Blob},
        eip7594::{BlobCellMask, BlobTransactionCellSidecar, BlobTransactionSidecarEip7594},
    };
    use std::sync::OnceLock;

    // A full sidecar with two different blobs, and all of its cells.
    pub(crate) fn fixture() -> &'static (BlobTransactionSidecarEip7594, BlobTransactionCellSidecar)
    {
        static FIXTURE: OnceLock<(BlobTransactionSidecarEip7594, BlobTransactionCellSidecar)> =
            OnceLock::new();
        FIXTURE.get_or_init(|| {
            let mut second = Blob::default();
            second[31] = 1;
            let full = BlobTransactionSidecarEip7594::try_from_blobs(vec![Blob::default(), second])
                .unwrap();
            let cells =
                BlobTransactionCellSidecar::try_from_sidecar(&full, EnvKzgSettings::Default.get())
                    .unwrap();
            (full, cells)
        })
    }

    // Keeps only the cells at `mask`.
    pub(crate) fn subset(mask: BlobCellMask) -> BlobTransactionCellSidecar {
        let (_, all) = fixture();
        BlobTransactionCellSidecar::new(
            all.commitments().to_vec(),
            all.proofs().to_vec(),
            all.matching_cells(mask).unwrap(),
            mask,
        )
        .unwrap()
    }
}
