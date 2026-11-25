//! [EIP-7594](https://eips.ethereum.org/EIPS/eip-7594) sidecars that keep only some of each blob's
//! cells.

use alloy_eips::{
    eip4844::{
        c_kzg, env_settings::KzgSettings, kzg_to_versioned_hash, AsCkzg,
        BlobTransactionValidationError, Bytes48,
    },
    eip7594::{
        BlobCellMask, BlobCellRecoveryError, BlobTransactionSidecarEip7594, Cell,
        CELLS_PER_EXT_BLOB,
    },
};
use alloy_primitives::B256;

/// An EIP-7594 sidecar that keeps the same subset of cells for every blob.
///
/// All commitments and all 128 proofs per blob are kept, so the transaction body can be served
/// without its blobs. Cells are ordered by blob, then by ascending cell index. The cell counts
/// always match the commitments and the mask, see [`Self::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobTxCellSidecar {
    // One commitment per blob.
    commitments: Vec<Bytes48>,
    // All 128 cell proofs of every blob, ordered by blob.
    proofs: Vec<Bytes48>,
    // The kept cells, ordered by blob, then by ascending cell index.
    cells: Vec<Cell>,
    // Indices of the cells kept for every blob.
    mask: BlobCellMask,
}

impl BlobTxCellSidecar {
    /// Creates a sidecar from its parts.
    ///
    /// Returns `None` unless there is at least one commitment and one cell index, `cells` holds
    /// the masked cells of every blob, and `proofs` holds all 128 proofs of every blob.
    pub fn new(
        commitments: Vec<Bytes48>,
        proofs: Vec<Bytes48>,
        cells: Vec<Cell>,
        mask: BlobCellMask,
    ) -> Option<Self> {
        let blobs = commitments.len();
        (blobs > 0 &&
            mask.count() > 0 &&
            blobs.checked_mul(mask.count()) == Some(cells.len()) &&
            blobs.checked_mul(CELLS_PER_EXT_BLOB) == Some(proofs.len()))
        .then_some(Self { commitments, proofs, cells, mask })
    }

    /// Computes every cell of a full sidecar.
    ///
    /// This does not check the sidecar's proofs; the caller validates the sidecar first.
    pub fn from_full(
        sidecar: &BlobTransactionSidecarEip7594,
        settings: &KzgSettings,
    ) -> Result<Self, c_kzg::Error> {
        let cells = sidecar.compute_cells_with_settings(settings)?;
        Self::new(
            sidecar.commitments.clone(),
            sidecar.cell_proofs.clone(),
            cells,
            BlobCellMask::from_bits(u128::MAX),
        )
        .ok_or_else(|| {
            c_kzg::Error::MismatchLength(format!(
                "{} blobs, {} commitments and {} cell proofs",
                sidecar.blobs.len(),
                sidecar.commitments.len(),
                sidecar.cell_proofs.len()
            ))
        })
    }

    /// Returns one commitment per blob.
    pub fn commitments(&self) -> &[Bytes48] {
        &self.commitments
    }

    /// Returns all 128 cell proofs of every blob, ordered by blob.
    pub fn proofs(&self) -> &[Bytes48] {
        &self.proofs
    }

    /// Returns the kept cells, ordered by blob, then by ascending cell index.
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Returns the indices of the cells kept for every blob.
    pub const fn mask(&self) -> BlobCellMask {
        self.mask
    }

    /// Returns whether enough cells are kept to rebuild every blob.
    pub const fn is_recoverable(&self) -> bool {
        self.mask.count() >= CELLS_PER_EXT_BLOB / 2
    }

    /// Returns the versioned hash of every commitment, in blob order.
    pub fn versioned_hashes(&self) -> impl Iterator<Item = B256> + '_ {
        self.commitments.iter().map(|commitment| kzg_to_versioned_hash(commitment.as_slice()))
    }

    /// Checks the commitments against `versioned_hashes` and every kept cell against its proof.
    pub fn validate(
        &self,
        versioned_hashes: &[B256],
        settings: &KzgSettings,
    ) -> Result<(), BlobTransactionValidationError> {
        if versioned_hashes.len() != self.commitments.len() {
            return Err(c_kzg::Error::MismatchLength(format!(
                "{} versioned hashes and {} commitments",
                versioned_hashes.len(),
                self.commitments.len()
            ))
            .into())
        }
        for (&expected, have) in versioned_hashes.iter().zip(self.versioned_hashes()) {
            if expected != have {
                return Err(BlobTransactionValidationError::WrongVersionedHash { have, expected })
            }
        }
        let mut commitments = Vec::with_capacity(self.cells.len());
        let mut proofs = Vec::with_capacity(self.cells.len());
        let mut indices = Vec::with_capacity(self.cells.len());
        for (blob, commitment) in self.commitments.iter().enumerate() {
            for index in self.mask.selected_indices() {
                commitments.push(*commitment);
                proofs.push(self.proofs[blob * CELLS_PER_EXT_BLOB + index]);
                indices.push(index as u64);
            }
        }
        if !settings.verify_cell_kzg_proof_batch(
            Bytes48::slice_as_ckzg(&commitments),
            &indices,
            Cell::slice_as_ckzg(&self.cells),
            Bytes48::slice_as_ckzg(&proofs),
        )? {
            return Err(BlobTransactionValidationError::InvalidProof)
        }
        Ok(())
    }

    /// Rebuilds the full sidecar, checking each rebuilt blob against its commitment.
    pub fn recover(
        &self,
        settings: &KzgSettings,
    ) -> Result<BlobTransactionSidecarEip7594, BlobCellRecoveryError> {
        BlobTransactionSidecarEip7594::try_recover_from_cells_with_settings(
            self.commitments.clone(),
            self.mask,
            &self.cells,
            settings,
        )
    }

    /// Returns the sidecar without blobs, as sent in eth/72 `PooledTransactions` responses.
    pub fn without_blobs(&self) -> BlobTransactionSidecarEip7594 {
        BlobTransactionSidecarEip7594::new(
            Vec::new(),
            self.commitments.clone(),
            self.proofs.clone(),
        )
    }

    /// Returns the cells at `mask` for every blob, or `None` if any of them is not kept.
    pub fn get_cells(&self, mask: BlobCellMask) -> Option<Vec<Cell>> {
        if mask.bits() & !self.mask.bits() != 0 {
            return None
        }
        let positions: Vec<_> = self
            .mask
            .selected_indices()
            .enumerate()
            .filter_map(|(position, index)| mask.contains(index).then_some(position))
            .collect();
        Some(
            self.cells
                .chunks_exact(self.mask.count())
                .flat_map(|blob| positions.iter().map(|&position| blob[position]))
                .collect(),
        )
    }

    /// Returns the heap size of the cells, commitments and proofs, in bytes, including spare
    /// capacity.
    pub const fn size(&self) -> usize {
        self.cells.capacity() * size_of::<Cell>() +
            (self.commitments.capacity() + self.proofs.capacity()) * size_of::<Bytes48>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip4844::{env_settings::EnvKzgSettings, Blob};
    use std::sync::OnceLock;

    // A full sidecar with two different blobs, and all of its cells.
    fn fixture() -> &'static (BlobTransactionSidecarEip7594, BlobTxCellSidecar) {
        static FIXTURE: OnceLock<(BlobTransactionSidecarEip7594, BlobTxCellSidecar)> =
            OnceLock::new();
        FIXTURE.get_or_init(|| {
            let mut second = Blob::default();
            second[31] = 1;
            let full = BlobTransactionSidecarEip7594::try_from_blobs(vec![Blob::default(), second])
                .unwrap();
            let cells = BlobTxCellSidecar::from_full(&full, settings()).unwrap();
            (full, cells)
        })
    }

    fn settings() -> &'static KzgSettings {
        EnvKzgSettings::Default.get()
    }

    // Keeps only the cells at `mask`.
    fn subset(mask: BlobCellMask) -> BlobTxCellSidecar {
        let (_, all) = fixture();
        BlobTxCellSidecar::new(
            all.commitments().to_vec(),
            all.proofs().to_vec(),
            all.get_cells(mask).unwrap(),
            mask,
        )
        .unwrap()
    }

    #[test]
    fn new_rejects_counts_that_do_not_match() {
        let (_, all) = fixture();
        let mask = BlobCellMask::from_bits(0b11);
        let cells = all.get_cells(mask).unwrap();
        let new = |commitments: &[Bytes48], proofs: &[Bytes48], cells: &[Cell], mask| {
            BlobTxCellSidecar::new(commitments.to_vec(), proofs.to_vec(), cells.to_vec(), mask)
        };
        assert!(new(all.commitments(), all.proofs(), &cells, mask).is_some());
        assert!(new(&[], &[], &[], mask).is_none());
        assert!(new(all.commitments(), all.proofs(), &[], BlobCellMask::default()).is_none());
        assert!(new(all.commitments(), all.proofs(), &cells[1..], mask).is_none());
        assert!(new(all.commitments(), &all.proofs()[1..], &cells, mask).is_none());
        assert!(new(&all.commitments()[1..], all.proofs(), &cells, mask).is_none());
    }

    #[test]
    fn recovers_blobs_from_half_of_the_columns() {
        let (full, all) = fixture();
        assert!(all.is_recoverable());
        assert_eq!(&all.recover(settings()).unwrap(), full);

        // Every other column, so the kept columns are not contiguous.
        let half = subset(BlobCellMask::from_bits(u128::MAX / 3));
        assert_eq!(half.mask().count(), CELLS_PER_EXT_BLOB / 2);
        assert!(half.is_recoverable());
        assert_eq!(&half.recover(settings()).unwrap(), full);

        let too_few = subset(BlobCellMask::from_bits(u64::MAX as u128 >> 1));
        assert!(!too_few.is_recoverable());
        assert!(matches!(
            too_few.recover(settings()),
            Err(BlobCellRecoveryError::InsufficientCells { provided: 63, required: 64 })
        ));
    }

    #[test]
    fn validate_rejects_bad_cells_proofs_and_hashes() {
        let valid = subset(BlobCellMask::from_bits((1 << 127) | 1));
        let hashes: Vec<_> = valid.versioned_hashes().collect();
        valid.validate(&hashes, settings()).unwrap();

        let mut bad_cell = valid.clone();
        bad_cell.cells[0][31] ^= 1;
        assert!(matches!(
            bad_cell.validate(&hashes, settings()),
            Err(BlobTransactionValidationError::InvalidProof)
        ));

        // The second blob's proof for the same column is a valid point but the wrong proof.
        let mut bad_proof = valid.clone();
        bad_proof.proofs.swap(0, CELLS_PER_EXT_BLOB);
        assert!(matches!(
            bad_proof.validate(&hashes, settings()),
            Err(BlobTransactionValidationError::InvalidProof)
        ));

        let mut wrong_hashes = hashes.clone();
        wrong_hashes[1] = B256::ZERO;
        assert!(matches!(
            valid.validate(&wrong_hashes, settings()),
            Err(BlobTransactionValidationError::WrongVersionedHash { have, expected })
                if have == hashes[1] && expected == B256::ZERO
        ));
        assert!(matches!(
            valid.validate(&hashes[..1], settings()),
            Err(BlobTransactionValidationError::KZGError(c_kzg::Error::MismatchLength(_)))
        ));
    }

    #[test]
    fn get_cells_returns_kept_columns_only() {
        let (_, all) = fixture();
        let kept = subset(BlobCellMask::from_bits(0b1011));
        let wanted = BlobCellMask::from_bits(0b1001);
        let expected = vec![all.cells[0], all.cells[3], all.cells[128], all.cells[131]];
        assert_eq!(kept.get_cells(wanted).unwrap(), expected);
        assert_eq!(kept.get_cells(BlobCellMask::from_bits(0b0100)), None);
    }

    #[test]
    fn without_blobs_keeps_commitments_and_proofs() {
        let (full, all) = fixture();
        let mut expected = full.clone();
        expected.blobs.clear();
        assert_eq!(subset(BlobCellMask::from_bits(1)).without_blobs(), expected);
        assert_eq!(
            all.size(),
            2 * CELLS_PER_EXT_BLOB * size_of::<Cell>() +
                (2 + 2 * CELLS_PER_EXT_BLOB) * size_of::<Bytes48>()
        );

        // Spare capacity is still allocated, so it is counted.
        let mask = BlobCellMask::from_bits(1);
        let mut cells = Vec::with_capacity(all.cells().len());
        cells.extend(all.get_cells(mask).unwrap());
        let capacity = cells.capacity();
        let sparse =
            BlobTxCellSidecar::new(all.commitments().to_vec(), all.proofs().to_vec(), cells, mask)
                .unwrap();
        assert_eq!(
            sparse.size(),
            capacity * size_of::<Cell>() + (2 + 2 * CELLS_PER_EXT_BLOB) * size_of::<Bytes48>()
        );
    }
}
