//! Ordered transaction announcements used by the transaction manager and fetcher.

use alloy_primitives::{map::B256Set, TxHash, B128};
use derive_more::IntoIterator;
use reth_eth_wire::{EthVersion, HandleMempoolData, NewPooledTransactionHashes};

/// An announcement with unique hashes in the order supplied by the peer.
///
/// Metadata comes from the wire message. Network-specific validation is performed by the
/// transaction manager's announcement policy.
#[derive(Debug, IntoIterator)]
pub struct TransactionAnnouncement {
    #[into_iterator(owned, ref)]
    entries: Vec<AnnouncedTransaction>,
    version: EthVersion,
    cell_mask: Option<B128>,
}

impl TransactionAnnouncement {
    /// Normalizes a wire announcement, keeping the first occurrence of each hash and its metadata.
    /// The scratch set is cleared before use, retaining its allocation for subsequent messages.
    ///
    /// Returns an error if the hash, type and size arrays have different lengths.
    pub fn from_message(
        msg: &NewPooledTransactionHashes,
        seen: &mut B256Set,
    ) -> alloy_rlp::Result<Self> {
        let (metadata, cell_mask) = match msg {
            NewPooledTransactionHashes::Eth66(_) => (None, None),
            NewPooledTransactionHashes::Eth68(msg) => {
                (Some((msg.types.as_slice(), msg.sizes.as_slice())), None)
            }
            NewPooledTransactionHashes::Eth72(msg) => {
                (Some((msg.types.as_slice(), msg.sizes.as_slice())), msg.cell_mask)
            }
        };
        if let Some((types, sizes)) = metadata {
            for len in [types.len(), sizes.len()] {
                if len != msg.len() {
                    return Err(alloy_rlp::Error::ListLengthMismatch {
                        expected: msg.len(),
                        got: len,
                    })
                }
            }
        }

        seen.clear();
        seen.reserve(msg.len());
        let mut entries = Vec::with_capacity(msg.len());
        entries.extend(msg.iter_hashes().enumerate().filter(|(_, hash)| seen.insert(**hash)).map(
            |(index, &hash)| AnnouncedTransaction {
                hash,
                metadata: metadata.map(|(types, sizes)| TransactionMetadata {
                    tx_type: types[index],
                    size: sizes[index],
                }),
            },
        ));
        Ok(Self { entries, version: msg.version(), cell_mask })
    }

    /// Returns the wire message version.
    pub const fn version(&self) -> EthVersion {
        self.version
    }

    /// Returns the eth/72 message-level cell mask, if present.
    pub const fn cell_mask(&self) -> Option<B128> {
        self.cell_mask
    }

    /// Returns the number of entries.
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether there are no entries.
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterates over the entries in announcement order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &AnnouncedTransaction> {
        self.entries.iter()
    }

    /// Retains entries satisfying the predicate without changing their order.
    pub fn retain(&mut self, f: impl FnMut(&AnnouncedTransaction) -> bool) {
        self.entries.retain(f);
    }
}

impl HandleMempoolData for TransactionAnnouncement {
    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    fn len(&self) -> usize {
        self.len()
    }

    fn retain_by_hash(&mut self, mut f: impl FnMut(&TxHash) -> bool) {
        self.retain(|tx| f(&tx.hash));
    }
}

/// A transaction hash and the metadata supplied by its announcing peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnouncedTransaction {
    /// The announced transaction hash.
    pub hash: TxHash,
    /// Type and size for eth/68 and later; absent for eth/66.
    pub metadata: Option<TransactionMetadata>,
}

/// Transaction metadata supplied in eth/68 and later announcements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransactionMetadata {
    /// The transaction type byte, interpreted by the network's announcement policy.
    pub tx_type: u8,
    /// The announced encoded transaction size in bytes.
    pub size: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_eth_wire::{NewPooledTransactionHashes68, NewPooledTransactionHashes72};

    #[test]
    fn dedup_preserves_first_metadata_order_and_version() {
        let hashes = [
            B256::repeat_byte(3),
            B256::repeat_byte(1),
            B256::repeat_byte(3),
            B256::repeat_byte(2),
        ];
        let types = vec![1, 2, 3, 4];
        let sizes = vec![100, 200, 300, 400];
        let mask = Some(B128::repeat_byte(0x11));
        let messages = [
            NewPooledTransactionHashes::Eth66(hashes.to_vec().into()),
            NewPooledTransactionHashes68 {
                hashes: hashes.to_vec(),
                types: types.clone(),
                sizes: sizes.clone(),
            }
            .into(),
            NewPooledTransactionHashes72 { hashes: hashes.to_vec(), types, sizes, cell_mask: mask }
                .into(),
        ];
        let mut seen = B256Set::default();
        for msg in messages {
            let mut announcement = TransactionAnnouncement::from_message(&msg, &mut seen).unwrap();
            assert_eq!(announcement.version(), msg.version());
            assert_eq!(
                announcement.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
                [hashes[0], hashes[1], hashes[3]]
            );
            let metadata = announcement.iter().map(|tx| tx.metadata).collect::<Vec<_>>();
            if msg.version().has_eth68_metadata() {
                assert_eq!(
                    metadata,
                    [(1, 100), (2, 200), (4, 400)]
                        .map(|(tx_type, size)| Some(TransactionMetadata { tx_type, size }))
                );
            } else {
                assert_eq!(metadata, [None; 3]);
            }

            // Both hash-only and entry filters must keep the remaining metadata aligned.
            announcement.retain_by_hash(|hash| *hash != hashes[1]);
            announcement.retain(|tx| tx.hash != hashes[0]);
            assert_eq!(announcement.iter().next().unwrap().hash, hashes[3]);
            assert_eq!(
                announcement.cell_mask(),
                if msg.version() == EthVersion::Eth72 { mask } else { None }
            );
        }
    }

    #[test]
    fn rejects_misaligned_metadata_in_locally_constructed_messages() {
        for (hashes_len, types_len, sizes_len) in
            [(2, 1, 2), (2, 2, 1), (1, 2, 1), (1, 1, 2), (0, 1, 1)]
        {
            let hashes = vec![B256::ZERO; hashes_len];
            let types = vec![2; types_len];
            let sizes = vec![100; sizes_len];
            for msg in [
                NewPooledTransactionHashes68 {
                    hashes: hashes.clone(),
                    types: types.clone(),
                    sizes: sizes.clone(),
                }
                .into(),
                NewPooledTransactionHashes72 { hashes, types, sizes, cell_mask: None }.into(),
            ] {
                assert!(
                    TransactionAnnouncement::from_message(&msg, &mut B256Set::default()).is_err()
                );
            }
        }
    }

    #[test]
    fn scratch_is_cleared_and_reused_between_messages() {
        let mut seen = B256Set::default();
        let msg = NewPooledTransactionHashes::Eth66(vec![B256::ZERO; 64].into());
        TransactionAnnouncement::from_message(&msg, &mut seen).unwrap();
        let capacity = seen.capacity();
        assert!(capacity >= 64);
        let second = TransactionAnnouncement::from_message(&msg, &mut seen).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(seen.capacity(), capacity);

        let large = NewPooledTransactionHashes::Eth66(vec![B256::repeat_byte(1); 16384].into());
        assert_eq!(TransactionAnnouncement::from_message(&large, &mut seen).unwrap().len(), 1);
        let capacity = seen.capacity();
        assert!(capacity >= 16384);
        assert_eq!(TransactionAnnouncement::from_message(&msg, &mut seen).unwrap().len(), 1);
        assert_eq!(seen.capacity(), capacity);
    }
}
