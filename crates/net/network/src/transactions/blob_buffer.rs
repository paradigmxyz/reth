//! Bounded rendezvous between ETH/72 transaction bodies and independently fetched cells.

use alloy_eips::{
    eip4844::env_settings::EnvKzgSettings,
    eip7594::{BlobCellMask, Cell},
};
use alloy_primitives::{map::B256Map, B128, B256};
use reth_network_peers::PeerId;
use reth_transaction_pool::{
    blobstore::{BlobTxCellSidecar, PooledBlobSidecar},
    PoolTransaction,
};
use std::time::{Duration, Instant};

/// Body and cell buffer. Cryptographic checks run after entries leave this buffer.
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

/// Maximum retained bodies, deliveries, and total cell memory.
const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(120);

impl<T: PoolTransaction> BlobBuffer<T> {
    pub(super) fn stats(&self) -> (usize, usize) {
        (self.entries.len(), self.bytes)
    }
    pub(super) fn expire(&mut self, now: Instant) -> Vec<B256> {
        let mut expired = Vec::new();
        self.entries.retain(|hash, entry| {
            if now.duration_since(entry.created) < TTL {
                return true
            }
            self.bytes -= entry.bytes;
            expired.push(*hash);
            false
        });
        expired
    }

    /// Rejects malformed metadata before reserving buffer memory. Stateful admission happens later.
    pub(super) fn body(&mut self, peer: PeerId, tx: T, now: Instant) -> Result<(), ()> {
        let sidecar = tx.blob_sidecar().and_then(|s| s.as_eip7594()).ok_or(())?;
        let hashes = tx.blob_versioned_hashes().ok_or(())?;
        if !sidecar.blobs.is_empty() ||
            hashes.is_empty() ||
            hashes.len() > 128 ||
            sidecar.commitments.len() != hashes.len() ||
            sidecar.cell_proofs.len() != hashes.len() * 128 ||
            sidecar
                .commitments
                .iter()
                .zip(hashes)
                .any(|(c, h)| alloy_eips::eip4844::kzg_to_versioned_hash(c.as_slice()) != *h) ||
            tx.max_priority_fee_per_gas().is_some_and(|tip| tip > tx.max_fee_per_gas())
        {
            return Err(())
        }
        let hash = *tx.hash();
        let size = tx.size();
        if self.bytes.saturating_add(size) > MAX_BYTES ||
            (!self.entries.contains_key(&hash) && self.entries.len() >= MAX_ENTRIES)
        {
            return Ok(())
        }
        let entry = self.entries.entry(hash).or_insert_with(|| Entry::new(now));
        if entry.body.is_none() {
            entry.body = Some((peer, tx));
            entry.bytes += size;
            self.bytes += size;
        }
        Ok(())
    }

    pub(super) fn cells(
        &mut self,
        hash: B256,
        peer: PeerId,
        mask: BlobCellMask,
        cells: Vec<Cell>,
        now: Instant,
    ) -> bool {
        let size = cells.len() * core::mem::size_of::<Cell>();
        if mask.count() == 0 ||
            cells.is_empty() ||
            !cells.len().is_multiple_of(mask.count()) ||
            cells.len() / mask.count() > 128 ||
            self.bytes.saturating_add(size) > MAX_BYTES ||
            (!self.entries.contains_key(&hash) && self.entries.len() >= MAX_ENTRIES)
        {
            return false
        }
        let entry = self.entries.entry(hash).or_insert_with(|| Entry::new(now));
        // Fetch requests reserve disjoint columns; overlapping replies are stale.
        if entry.mask & mask.bits() != 0 {
            return false
        }
        entry.mask |= mask.bits();
        entry.deliveries.push(Delivery { peer, mask, cells });
        entry.bytes += size;
        self.bytes += size;
        true
    }

    pub(super) fn take_ready(
        &mut self,
        hash: B256,
        target: BlobCellMask,
    ) -> Option<BufferedBlob<T>> {
        let entry = self.entries.get(&hash)?;
        if entry.body.is_none() ||
            entry.mask & target.bits() != target.bits() ||
            target.count() == 0
        {
            return None
        }
        let entry = self.entries.remove(&hash)?;
        self.bytes -= entry.bytes;
        let (peer, transaction) = entry.body?;
        Some(BufferedBlob { peer, transaction, deliveries: entry.deliveries })
    }
}

/// Owned verification job. Keeps provenance until every peer's delivery has been checked.
pub(super) struct BufferedBlob<T> {
    pub(super) peer: PeerId,
    transaction: T,
    deliveries: Vec<Delivery>,
}

impl<T: PoolTransaction> BufferedBlob<T> {
    pub(super) fn verify(mut self) -> Result<(PeerId, T), Vec<PeerId>> {
        let metadata = self
            .transaction
            .blob_sidecar()
            .and_then(|s| s.as_eip7594())
            .expect("buffer validates metadata");
        let hashes = self.transaction.blob_versioned_hashes().expect("blob transaction");
        let mut bad = Vec::new();
        let mut verified = Vec::new();
        let mut mask = 0;
        for delivery in self.deliveries {
            let sidecar = BlobTxCellSidecar {
                commitments: metadata.commitments.clone(),
                proofs: metadata.cell_proofs.clone(),
                cells: delivery.cells,
                cell_mask: B128::from(delivery.mask.bits()),
            };
            if sidecar.validate(hashes, EnvKzgSettings::Default.get()).is_err() {
                if !bad.contains(&delivery.peer) {
                    bad.push(delivery.peer);
                }
            } else {
                mask |= delivery.mask.bits();
                verified.push(sidecar);
            }
        }
        if !bad.is_empty() {
            return Err(bad)
        }
        let mask = BlobCellMask::from_bits(mask);
        let mut cells = Vec::with_capacity(metadata.commitments.len() * mask.count());
        for blob in 0..metadata.commitments.len() {
            for column in mask.selected_indices() {
                let source = verified
                    .iter()
                    .find(|s| s.mask().contains(column))
                    .expect("union of verified masks");
                let offset = source.mask().selected_indices().position(|i| i == column).unwrap();
                cells.push(source.cells[blob * source.mask().count() + offset]);
            }
        }
        let sidecar = BlobTxCellSidecar {
            commitments: metadata.commitments.clone(),
            proofs: metadata.cell_proofs.clone(),
            cells,
            cell_mask: B128::from(mask.bits()),
        };
        if !self.transaction.set_blob_sidecar(PooledBlobSidecar::from_cells(sidecar)) {
            return Err(Vec::new())
        }
        Ok((self.peer, self.transaction))
    }
}

#[derive(Debug)]
struct Entry<T> {
    created: Instant,
    body: Option<(PeerId, T)>,
    deliveries: Vec<Delivery>,
    mask: u128,
    bytes: usize,
}
impl<T> Entry<T> {
    const fn new(created: Instant) -> Self {
        Self { created, body: None, deliveries: Vec::new(), mask: 0, bytes: 0 }
    }
}
#[derive(Debug)]
struct Delivery {
    peer: PeerId,
    mask: BlobCellMask,
    cells: Vec<Cell>,
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use alloy_consensus::{Signed, TxEip4844, TxEip4844WithSidecar};
    use alloy_eips::{eip4844::Blob, eip7594::BlobTransactionSidecarEip7594};
    use alloy_primitives::{Address, Signature};
    use reth_ethereum_primitives::PooledTransactionVariant;
    use reth_primitives_traits::Recovered;
    use reth_transaction_pool::EthPooledTransaction;

    pub(in crate::transactions) fn fixture() -> (EthPooledTransaction, BlobTxCellSidecar) {
        let full = BlobTransactionSidecarEip7594::try_from_blobs(vec![Blob::default()]).unwrap();
        let cells = BlobTxCellSidecar::from_full(&full, EnvKzgSettings::Default.get()).unwrap();
        let tx = TxEip4844 {
            blob_versioned_hashes: cells.versioned_hashes().collect(),
            gas_limit: 21000,
            ..Default::default()
        };
        let tx = Signed::new_unhashed(
            TxEip4844WithSidecar { tx, sidecar: cells.elided().into() },
            Signature::test_signature(),
        );
        let pooled = PooledTransactionVariant::Eip4844(tx);
        (EthPooledTransaction::from_pooled(Recovered::new_unchecked(pooled, Address::ZERO)), cells)
    }

    #[test]
    fn rendezvous_in_either_order_and_partial_custody() {
        for body_first in [true, false] {
            let (tx, cells) = fixture();
            let hash = *tx.hash();
            let peer = PeerId::random();
            let mask = BlobCellMask::from_bits((1 << 127) | 1);
            let now = Instant::now();
            let mut buffer = BlobBuffer::default();
            if body_first {
                buffer.body(peer, tx.clone(), now).unwrap();
            }
            assert!(buffer.cells(hash, peer, mask, cells.get_cells(mask).unwrap(), now));
            if !body_first {
                assert!(buffer.take_ready(hash, mask).is_none());
                buffer.body(peer, tx, now).unwrap();
            }
            let (_, tx) = buffer.take_ready(hash, mask).unwrap().verify().unwrap();
            assert_eq!(tx.blob_cell_availability().unwrap().get(), mask);
            assert!(!tx.blob_cell_availability().unwrap().is_recoverable());
            assert_eq!(buffer.bytes, 0);
        }
    }

    #[test]
    fn only_bad_cell_provider_is_reported() {
        let (tx, cells) = fixture();
        let hash = *tx.hash();
        let body_peer = PeerId::random();
        let honest = PeerId::random();
        let bad = PeerId::random();
        let now = Instant::now();
        let mut buffer = BlobBuffer::default();
        buffer.body(body_peer, tx, now).unwrap();
        let first = BlobCellMask::from_bits(1);
        let second = BlobCellMask::from_bits(2);
        buffer.cells(hash, honest, first, cells.get_cells(first).unwrap(), now);
        let mut corrupted = cells.get_cells(second).unwrap();
        corrupted[0][31] ^= 1;
        buffer.cells(hash, bad, second, corrupted, now);
        let result = buffer.take_ready(hash, BlobCellMask::from_bits(3)).unwrap().verify();
        assert_eq!(result.unwrap_err(), vec![bad]);
    }

    #[test]
    fn buffer_expiry_and_overlapping_deliveries() {
        let (tx, cells) = fixture();
        let hash = *tx.hash();
        let peer = PeerId::random();
        let mask = BlobCellMask::from_bits(1);
        let now = Instant::now();
        let mut buffer = BlobBuffer::default();
        buffer.body(peer, tx, now).unwrap();
        assert!(buffer.cells(hash, peer, mask, cells.get_cells(mask).unwrap(), now));
        let bytes = buffer.bytes;
        assert!(!buffer.cells(hash, peer, mask, cells.get_cells(mask).unwrap(), now));
        assert_eq!(bytes, buffer.bytes);
        assert_eq!(buffer.expire(now + TTL), vec![hash]);
        assert_eq!(buffer.bytes, 0);
        assert!(buffer.take_ready(hash, mask).is_none());
    }
}
