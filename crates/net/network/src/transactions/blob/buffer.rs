//! Bounded rendezvous between eth/72 transaction bodies and their separately fetched cells.

use alloy_eips::{
    eip4844::{env_settings::EnvKzgSettings, kzg_to_versioned_hash},
    eip7594::{BlobCellMask, Cell, CELLS_PER_EXT_BLOB},
};
use alloy_primitives::{map::B256Map, B256};
use reth_network_peers::PeerId;
use reth_transaction_pool::{
    blobstore::{BlobTxCellSidecar, PooledBlobSidecar},
    PoolTransaction,
};
use std::time::{Duration, Instant};

/// Most transactions buffered while their body or cells are missing.
const MAX_BUFFERED_TRANSACTIONS: usize = 256;

/// Most bytes of bodies and cells buffered.
const MAX_BUFFERED_BYTES: usize = 64 * 1024 * 1024;

/// How long an incomplete transaction stays buffered.
const BUFFER_TTL: Duration = Duration::from_secs(120);

/// Holds eth/72 blob transaction bodies, which arrive without blobs, until their cells arrive.
///
/// Cells are checked only once an entry leaves the buffer, see [`BufferedBlob::verify`].
#[derive(Debug)]
pub(super) struct BlobBuffer<T> {
    entries: B256Map<Entry<T>>,
    bytes: usize,
}

impl<T> Default for BlobBuffer<T> {
    fn default() -> Self {
        Self { entries: Default::default(), bytes: 0 }
    }
}

impl<T: PoolTransaction> BlobBuffer<T> {
    /// Returns the number of buffered transactions and their bytes.
    pub(super) fn stats(&self) -> (usize, usize) {
        (self.entries.len(), self.bytes)
    }

    /// Removes the entries buffered for longer than [`BUFFER_TTL`] and returns how many.
    pub(super) fn expire(&mut self, now: Instant) -> usize {
        let len = self.entries.len();
        self.entries.retain(|_, entry| {
            let keep = now.duration_since(entry.created) < BUFFER_TTL;
            if !keep {
                self.bytes -= entry.bytes;
            }
            keep
        });
        len - self.entries.len()
    }

    /// Buffers the body of `tx` sent by `peer`, keeping the first body of each transaction.
    ///
    /// Returns `false` if the body can't belong to an eth/72 blob transaction, so the peer sent
    /// a malformed body. A body is dropped without error when the buffer is full.
    pub(super) fn body(&mut self, peer: PeerId, tx: T, now: Instant) -> bool {
        let Some(sidecar) = tx.blob_sidecar().and_then(|sidecar| sidecar.as_eip7594()) else {
            return false
        };
        let Some(hashes) = tx.blob_versioned_hashes() else { return false };
        // The commitments are bound to the transaction hash through the versioned hashes, while
        // the proofs are only checked against the cells.
        if !sidecar.blobs.is_empty() ||
            hashes.is_empty() ||
            sidecar.commitments.len() != hashes.len() ||
            sidecar.cell_proofs.len() != hashes.len() * CELLS_PER_EXT_BLOB ||
            sidecar
                .commitments
                .iter()
                .zip(hashes)
                .any(|(commitment, hash)| kzg_to_versioned_hash(commitment.as_slice()) != *hash)
        {
            return false
        }
        let hash = *tx.hash();
        let size = tx.size();
        if !self.has_room(&hash, size) {
            return true
        }
        let entry = self.entries.entry(hash).or_insert_with(|| Entry::new(now));
        if entry.body.is_none() {
            entry.body = Some((peer, tx));
            entry.bytes += size;
            self.bytes += size;
        }
        true
    }

    /// Buffers the `cells` of `hash` at the columns in `mask`.
    ///
    /// Returns `false` if they were dropped: the buffer is full, or they overlap columns already
    /// buffered, which only a stale reply can do since requests ask for missing columns.
    pub(super) fn cells(
        &mut self,
        hash: B256,
        mask: BlobCellMask,
        cells: Vec<Cell>,
        now: Instant,
    ) -> bool {
        let size = cells.len() * size_of::<Cell>();
        if mask.count() == 0 ||
            cells.is_empty() ||
            !cells.len().is_multiple_of(mask.count()) ||
            !self.has_room(&hash, size)
        {
            return false
        }
        let entry = self.entries.entry(hash).or_insert_with(|| Entry::new(now));
        if entry.mask & mask.bits() != 0 {
            return false
        }
        entry.mask |= mask.bits();
        entry.deliveries.push(Delivery { mask, cells });
        entry.bytes += size;
        self.bytes += size;
        true
    }

    /// Removes and returns the entry of `hash` once it has a body and every column in `target`.
    pub(super) fn take_ready(
        &mut self,
        hash: B256,
        target: BlobCellMask,
    ) -> Option<BufferedBlob<T>> {
        let entry = self.entries.get(&hash)?;
        if entry.body.is_none() ||
            target.count() == 0 ||
            entry.mask & target.bits() != target.bits()
        {
            return None
        }
        let entry = self.entries.remove(&hash)?;
        self.bytes -= entry.bytes;
        let (peer, transaction) = entry.body?;
        Some(BufferedBlob { peer, transaction, deliveries: entry.deliveries })
    }

    // Returns whether `size` more bytes for `hash` fit in the buffer.
    fn has_room(&self, hash: &B256, size: usize) -> bool {
        self.bytes.saturating_add(size) <= MAX_BUFFERED_BYTES &&
            (self.entries.contains_key(hash) || self.entries.len() < MAX_BUFFERED_TRANSACTIONS)
    }
}

/// A transaction body with all the cells to fetch, taken out of the [`BlobBuffer`] for checking.
#[derive(Debug)]
pub(super) struct BufferedBlob<T> {
    peer: PeerId,
    transaction: T,
    deliveries: Vec<Delivery>,
}

impl<T: PoolTransaction> BufferedBlob<T> {
    /// Checks the cells against the body's commitments and proofs, and attaches them to it.
    ///
    /// Returns the body's sender and the transaction, or `None` if the cells or the body's proofs
    /// are invalid. This runs KZG checks, so it must run off the async threads.
    pub(super) fn verify(mut self) -> Option<(PeerId, T)> {
        // `BlobBuffer::body` only buffers bodies with an EIP-7594 sidecar.
        let metadata = self.transaction.blob_sidecar()?.as_eip7594()?;
        let hashes = self.transaction.blob_versioned_hashes()?;
        // Replies have disjoint masks, see `BlobBuffer::cells`, so each column has one source.
        let mask = BlobCellMask::from_bits(
            self.deliveries.iter().fold(0, |mask, delivery| mask | delivery.mask.bits()),
        );
        let mut cells = Vec::with_capacity(metadata.commitments.len() * mask.count());
        for blob in 0..metadata.commitments.len() {
            for column in mask.selected_indices() {
                let delivery =
                    self.deliveries.iter().find(|delivery| delivery.mask.contains(column))?;
                let position =
                    delivery.mask.selected_indices().position(|index| index == column)?;
                cells.push(*delivery.cells.get(blob * delivery.mask.count() + position)?);
            }
        }
        let sidecar = BlobTxCellSidecar::new(
            metadata.commitments.clone(),
            metadata.cell_proofs.clone(),
            cells,
            mask,
        )?;
        sidecar.validate(hashes, EnvKzgSettings::Default.get()).ok()?;
        self.transaction.set_blob_sidecar(PooledBlobSidecar::from_cells(sidecar)).then_some(())?;
        Some((self.peer, self.transaction))
    }
}

// A transaction waiting for its body or cells.
#[derive(Debug)]
struct Entry<T> {
    // When the first part arrived.
    created: Instant,
    // The first body received and its sender.
    body: Option<(PeerId, T)>,
    // Cells received, with disjoint masks.
    deliveries: Vec<Delivery>,
    // Columns of all deliveries.
    mask: u128,
    // Bytes of the body and the cells.
    bytes: usize,
}

impl<T> Entry<T> {
    const fn new(created: Instant) -> Self {
        Self { created, body: None, deliveries: Vec::new(), mask: 0, bytes: 0 }
    }
}

// The cells of one `Cells` reply, for every blob at the columns in `mask`.
#[derive(Debug)]
struct Delivery {
    mask: BlobCellMask,
    cells: Vec<Cell>,
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use alloy_consensus::{Signed, TxEip4844, TxEip4844WithSidecar};
    use alloy_eips::{
        eip4844::Blob,
        eip7594::{BlobTransactionSidecarEip7594, BlobTransactionSidecarVariant},
    };
    use alloy_primitives::{Address, Signature};
    use reth_ethereum_primitives::PooledTransactionVariant;
    use reth_primitives_traits::Recovered;
    use reth_transaction_pool::EthPooledTransaction;

    /// Returns an eth/72 body without blobs and all the cells of its blob.
    pub(in crate::transactions) fn fixture() -> (EthPooledTransaction, BlobTxCellSidecar) {
        let full = BlobTransactionSidecarEip7594::try_from_blobs(vec![Blob::default()]).unwrap();
        let cells = BlobTxCellSidecar::from_full(&full, EnvKzgSettings::Default.get()).unwrap();
        let tx = TxEip4844 {
            blob_versioned_hashes: cells.versioned_hashes().collect(),
            gas_limit: 21000,
            ..Default::default()
        };
        let tx = Signed::new_unhashed(
            TxEip4844WithSidecar { tx, sidecar: cells.without_blobs().into() },
            Signature::test_signature(),
        );
        let pooled = PooledTransactionVariant::Eip4844(tx);
        let tx = EthPooledTransaction::from_pooled(Recovered::new_unchecked(pooled, Address::ZERO));
        (tx, cells)
    }

    #[test]
    fn body_and_cells_meet_in_either_order() {
        for body_first in [true, false] {
            let (tx, cells) = fixture();
            let hash = *tx.hash();
            let peer = PeerId::random();
            let mask = BlobCellMask::from_bits(u64::MAX.into());
            let now = Instant::now();
            let mut buffer = BlobBuffer::default();
            if body_first {
                assert!(buffer.body(peer, tx.clone(), now));
            }
            assert!(buffer.cells(hash, mask, cells.get_cells(mask).unwrap(), now));
            if !body_first {
                assert!(buffer.take_ready(hash, mask).is_none());
                assert!(buffer.body(peer, tx, now));
            }
            let (sender, tx) = buffer.take_ready(hash, mask).unwrap().verify().unwrap();
            assert_eq!(sender, peer);
            assert_eq!(tx.blob_cell_availability().unwrap().get(), mask);
            assert_eq!(buffer.stats(), (0, 0));
        }
    }

    #[test]
    fn cells_from_several_replies_are_merged() {
        let (tx, cells) = fixture();
        let hash = *tx.hash();
        let now = Instant::now();
        let mut buffer = BlobBuffer::default();
        assert!(buffer.body(PeerId::random(), tx, now));
        let (low, high) = (BlobCellMask::from_bits(0b01), BlobCellMask::from_bits(0b10));
        assert!(buffer.cells(hash, high, cells.get_cells(high).unwrap(), now));
        assert!(buffer.cells(hash, low, cells.get_cells(low).unwrap(), now));
        let (_, tx) =
            buffer.take_ready(hash, BlobCellMask::from_bits(0b11)).unwrap().verify().unwrap();
        let sidecar = tx.blob_sidecar().and_then(PooledBlobSidecar::cells).unwrap();
        assert_eq!(sidecar.cells(), cells.get_cells(BlobCellMask::from_bits(0b11)).unwrap());
    }

    #[test]
    fn corrupted_cells_fail_verification() {
        let (tx, cells) = fixture();
        let hash = *tx.hash();
        let mask = BlobCellMask::from_bits(1);
        let now = Instant::now();
        let mut buffer = BlobBuffer::default();
        assert!(buffer.body(PeerId::random(), tx, now));
        let mut corrupted = cells.get_cells(mask).unwrap();
        corrupted[0][31] ^= 1;
        assert!(buffer.cells(hash, mask, corrupted, now));
        assert!(buffer.take_ready(hash, mask).unwrap().verify().is_none());
    }

    #[test]
    fn malformed_bodies_are_rejected() {
        let (tx, cells) = fixture();
        let mut buffer = BlobBuffer::default();
        let now = Instant::now();
        // A body with its blobs isn't an eth/72 body.
        let mut with_blobs = tx.clone();
        let full = BlobTransactionSidecarEip7594::try_from_blobs(vec![Blob::default()]).unwrap();
        assert!(with_blobs.set_blob_sidecar(PooledBlobSidecar::from(
            BlobTransactionSidecarVariant::Eip7594(full)
        )));
        assert!(!buffer.body(PeerId::random(), with_blobs, now));
        // Commitments must match the versioned hashes.
        let mut wrong_commitment = tx;
        let mut sidecar = cells.without_blobs();
        sidecar.commitments[0] = Default::default();
        assert!(wrong_commitment.set_blob_sidecar(PooledBlobSidecar::from(
            BlobTransactionSidecarVariant::Eip7594(sidecar)
        )));
        assert!(!buffer.body(PeerId::random(), wrong_commitment, now));
        assert_eq!(buffer.stats(), (0, 0));
    }

    #[test]
    fn expired_and_overlapping_cells_are_dropped() {
        let (tx, cells) = fixture();
        let hash = *tx.hash();
        let peer = PeerId::random();
        let mask = BlobCellMask::from_bits(1);
        let now = Instant::now();
        let mut buffer = BlobBuffer::default();
        assert!(buffer.body(peer, tx, now));
        assert!(buffer.cells(hash, mask, cells.get_cells(mask).unwrap(), now));
        let stats = buffer.stats();
        assert!(!buffer.cells(hash, mask, cells.get_cells(mask).unwrap(), now));
        assert_eq!(buffer.stats(), stats);
        assert_eq!(buffer.expire(now + BUFFER_TTL), 1);
        assert_eq!(buffer.stats(), (0, 0));
        assert!(buffer.take_ready(hash, mask).is_none());
    }
}
