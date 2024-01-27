//! Validation of [`NewPooledTransactionHashes68`] entries. These are of the form
//! `(ty, hash, size)`. Validation and filtering of entries is network dependent. [``]

use derive_more::{Deref, DerefMut, Display};
use itertools::izip;
use reth_eth_wire::NewPooledTransactionHashes68;
use reth_primitives::{TxHash, TxType};
use tracing::{debug, trace};

/// Interface for validating a `(ty, size, hash)` tuple from a [`NewPooledTransactionHashes68`].
pub trait ValidateTx68 {
    /// Validates a [`NewPooledTransactionHashes68`] entry. Returns [`ValidationOutcome`] which
    /// signals to the caller wether to fetch the transaction or wether to drop it, and wether the
    /// sender of the announcement should be penalized.
    fn should_fetch(&self, ty: u8, hash: TxHash, size: usize) -> ValidationOutcome;
}

/// Outcomes from validating a `(ty, hash, size)` entry from a [`NewPooledTransactionHashes68`].
/// Signals to the caller how to deal with an announcement entry and the peer who sent the
/// announcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationOutcome {
    /// Tells the caller to keep the entry in the announcement for fetch.
    Fetch,
    /// Tells the caller to filter out the entry from the announcement.
    Ignore,
    /// Tells the caller to filter out the entry from the announcement and penalize the peer. On
    /// this outcome, caller can drop the announcement, that is up to each implementation.
    ReportPeer,
}

/// Filters valid entires in [`NewPooledTransactionHashes68`] in place, and flags misbehaving
/// peers.
pub trait FilterAnnouncement68: ValidateTx68 {
    /// Removes invalid entries from a [`NewPooledTransactionHashes68`] announcement. Returns
    /// [`FilterOutcome::ReportPeer`] if the caller should penalize the peer, otherwise
    /// [`FilterOutcome::Ok`].
    fn filter_valid_entries(&self, msg: &mut NewPooledTransactionHashes68) -> FilterOutcome;
}

/// Outcome from filtering [`NewPooledTransactionHashes68`]. Signals to caller wether to penalize
/// the sender of the announcement or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOutcome {
    /// Peer behaves appropriately.
    Ok,
    /// A penalty should be flagged for the peer. Peer sent an announcement with unacceptably
    /// invalid entries.
    ReportPeer,
}

/// Wrapper for types that implement [`FilterAnnouncement68`]. The definition of a valid
/// announcement is network dependent. For example, different networks support different [`TxType`]
/// s, and different [`TxType`]s have different transaction size constraints. Defaults to
/// [`LayerOne`].
#[derive(Debug, Default, Deref, DerefMut)]
pub struct Announcement68Filter<N = LayerOne>(N);

/// L1 Network.
#[derive(Debug, Display, Default)]
#[non_exhaustive]
pub struct LayerOne;

impl ValidateTx68 for LayerOne {
    fn should_fetch(&self, ty: u8, hash: TxHash, size: usize) -> ValidationOutcome {
        // 1. checks if tx type is valid value for this network
        let tx_type = match TxType::try_from(ty) {
            Ok(ty) => ty,
            Err(_) => {
                debug!(target: "net::eth-wire",
                    ty=ty,
                    size=size,
                    hash=%hash,
                    network=%Self,
                    "invalid tx type in eth68 announcement"
                );

                return ValidationOutcome::ReportPeer
            }
        };
        // 2. checks if tx's encoded length is within limits for this network
        if size > tx_type.max_encoded_tx_length() || size < tx_type.min_encoded_tx_length() {
            // todo: min valid tx encoded length on layer 1? signature length?
            debug!(target: "net::eth-wire",
                ty=ty,
                size=size,
                hash=%hash,
                max_encoded_tx_length=tx_type.max_encoded_tx_length(),
                network=%Self,
                "invalid tx size in eth68 announcement"
            );

            return ValidationOutcome::Ignore
        }

        ValidationOutcome::Fetch
    }
}

impl FilterAnnouncement68 for LayerOne
where
    Self: ValidateTx68,
{
    fn filter_valid_entries(&self, msg: &mut NewPooledTransactionHashes68) -> FilterOutcome {
        trace!(target: "net::tx::validation",
            types=?msg.types,
            sizes=?msg.sizes,
            hashes=?msg.hashes,
            network=%Self,
            "validating eth68 announcement data.."
        );

        // 1. checks if the announcement is empty
        if msg.hashes.is_empty() {
            debug!(target: "net::tx",
                network=%Self,
                "empty announcement"
            );
            return FilterOutcome::ReportPeer
        }

        let mut should_report_peer = false;
        let mut indices_to_remove = vec![];

        for (i, (&ty, &hash, &size)) in izip!(&msg.types, &msg.hashes, &msg.sizes).enumerate() {
            match self.should_fetch(ty, hash, size) {
                ValidationOutcome::Fetch => (),
                ValidationOutcome::Ignore => indices_to_remove.push(i),
                ValidationOutcome::ReportPeer => {
                    indices_to_remove.push(i);
                    should_report_peer = true;
                }
            }
        }

        for (i, &index) in indices_to_remove.iter().rev().enumerate() {
            let index = index.saturating_sub(i);

            msg.hashes.remove(index);
            msg.types.remove(index);
            msg.sizes.remove(index);
        }

        let mut hashes_sorted = msg.hashes.clone();
        hashes_sorted.sort();

        for hash_pair in hashes_sorted.windows(2) {
            if hash_pair[0] == hash_pair[1] {
                let index = msg.hashes.iter().position(|&hash| hash == hash_pair[0]);
                if let Some(i) = index {
                    debug!(target: "net::eth-wire",
                        ty=msg.types[i],
                        size=msg.sizes[i],
                        hash=%msg.hashes[i],
                        network=%Self,
                        "duplicate tx hash in eth68 announcement"
                    );

                    msg.hashes.remove(i);
                    msg.types.remove(i);
                    msg.sizes.remove(i);
                }

                should_report_peer = true;
            }
        }

        if should_report_peer {
            return FilterOutcome::ReportPeer
        }

        FilterOutcome::Ok
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use reth_primitives::B256;

    use super::*;

    #[test]
    fn eth68_empty_announcement() {
        let types = vec![];
        let sizes = vec![];
        let hashes = vec![];

        let mut announcement = NewPooledTransactionHashes68 { types, sizes, hashes };

        let filter: Announcement68Filter = Announcement68Filter::default();

        let outcome = filter.filter_valid_entries(&mut announcement);

        assert_eq!(outcome, FilterOutcome::ReportPeer);
    }

    #[test]
    fn eth68_announcement_unrecognized_tx_type() {
        let types = vec![
            TxType::MAX_RESERVED_EIP as u8 + 1, // the first type isn't valid
            TxType::Legacy as u8,
        ];
        let sizes = vec![
            TxType::MAX_RESERVED_EIP.max_encoded_tx_length(),
            TxType::MAX_RESERVED_EIP.max_encoded_tx_length(),
        ];
        let hashes = vec![
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefbbbb")
                .unwrap(),
        ];

        let mut announcement = NewPooledTransactionHashes68 {
            types: types.clone(),
            sizes: sizes.clone(),
            hashes: hashes.clone(),
        };

        let filter: Announcement68Filter = Announcement68Filter::default();

        let outcome = filter.filter_valid_entries(&mut announcement);

        assert_eq!(outcome, FilterOutcome::ReportPeer);

        assert_eq!(
            announcement,
            NewPooledTransactionHashes68 {
                types: vec!(types[1]),
                sizes: vec!(sizes[1]),
                hashes: vec!(hashes[1]),
            }
        );
    }

    #[test]
    fn eth68_announcement_too_big_tx() {
        let types =
            vec![TxType::MAX_RESERVED_EIP as u8, TxType::Legacy as u8, TxType::EIP2930 as u8];
        let sizes = vec![
            TxType::MAX_RESERVED_EIP.max_encoded_tx_length(),
            TxType::MAX_RESERVED_EIP.max_encoded_tx_length() + 1, // the second length isn't valid
            TxType::MAX_RESERVED_EIP.max_encoded_tx_length() - 1,
        ];
        let hashes = vec![
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefbbbb")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefbbbc")
                .unwrap(),
        ];

        let mut announcement = NewPooledTransactionHashes68 {
            types: types.clone(),
            sizes: sizes.clone(),
            hashes: hashes.clone(),
        };

        let filter: Announcement68Filter = Announcement68Filter::default();

        let outcome = filter.filter_valid_entries(&mut announcement);

        assert_eq!(outcome, FilterOutcome::Ok);

        assert_eq!(
            announcement,
            NewPooledTransactionHashes68 {
                types: vec!(types[0], types[2]),
                sizes: vec!(sizes[0], sizes[2]),
                hashes: vec!(hashes[0], hashes[2]),
            }
        );
    }

    #[test]
    fn eth68_announcement_too_small_tx() {
        let types =
            vec![TxType::MAX_RESERVED_EIP as u8, TxType::Legacy as u8, TxType::EIP2930 as u8];
        let sizes = vec![
            TxType::MAX_RESERVED_EIP.min_encoded_tx_length() - 1, // the first length isn't valid
            TxType::MAX_RESERVED_EIP.min_encoded_tx_length() - 1, // neither is the second
            TxType::MAX_RESERVED_EIP.max_encoded_tx_length(),
        ];
        let hashes = vec![
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefbbbb")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeef00bb")
                .unwrap(),
        ];

        let mut announcement = NewPooledTransactionHashes68 {
            types: types.clone(),
            sizes: sizes.clone(),
            hashes: hashes.clone(),
        };

        let filter: Announcement68Filter = Announcement68Filter::default();

        let outcome = filter.filter_valid_entries(&mut announcement);

        assert_eq!(outcome, FilterOutcome::Ok);

        assert_eq!(
            announcement,
            NewPooledTransactionHashes68 {
                types: vec!(types[2]),
                sizes: vec!(sizes[2]),
                hashes: vec!(hashes[2]),
            }
        );
    }

    #[test]
    fn eth68_announcement_duplicate_tx_hash() {
        let types = vec![
            TxType::EIP1559 as u8,
            TxType::EIP4844 as u8,
            TxType::EIP1559 as u8,
            TxType::EIP4844 as u8,
        ];
        let sizes = vec![
            TxType::MAX_RESERVED_EIP.max_encoded_tx_length() - 3,
            TxType::MAX_RESERVED_EIP.min_encoded_tx_length() + 1,
            TxType::MAX_RESERVED_EIP.max_encoded_tx_length() - 1,
            TxType::MAX_RESERVED_EIP.min_encoded_tx_length() + 4,
        ];
        // first three or the same
        let hashes = vec![
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa") // removed dup
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa") // removed dup
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa") // dup
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefbbbb")
                .unwrap(),
        ];

        let mut announcement = NewPooledTransactionHashes68 {
            types: types.clone(),
            sizes: sizes.clone(),
            hashes: hashes.clone(),
        };

        let filter: Announcement68Filter = Announcement68Filter::default();

        let outcome = filter.filter_valid_entries(&mut announcement);

        assert_eq!(outcome, FilterOutcome::ReportPeer);

        //

        assert_eq!(
            announcement,
            NewPooledTransactionHashes68 {
                types: vec!(types[2], types[3]),
                sizes: vec!(sizes[2], sizes[3]),
                hashes: vec!(hashes[2], hashes[3]),
            }
        );
    }
}
