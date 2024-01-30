//! Validation of [`NewPooledTransactionHashes66`] and [`NewPooledTransactionHashes68`]
//! announcements. Validation and filtering of announcements is network dependent.

use std::{collections::HashMap, mem};

use derive_more::{Deref, DerefMut, Display};
use itertools::izip;
use reth_eth_wire::{
    NewPooledTransactionHashes66, NewPooledTransactionHashes68, ValidAnnouncementData,
    MAX_MESSAGE_SIZE,
};
use reth_primitives::{Signature, TxHash, TxType};
use tracing::{debug, trace};

/// The size of a decoded signature in bytes.
pub const SIGNATURE_DECODED_SIZE_BYTES: usize = mem::size_of::<Signature>();

/// Interface for validating a `(ty, size, hash)` tuple from a [`NewPooledTransactionHashes68`].
pub trait ValidateTx68 {
    /// Validates a [`NewPooledTransactionHashes68`] entry. Returns [`ValidationOutcome`] which
    /// signals to the caller wether to fetch the transaction or wether to drop it, and wether the
    /// sender of the announcement should be penalized.
    fn should_fetch(&self, ty: u8, hash: TxHash, size: usize) -> ValidationOutcome;

    /// Returns the reasonable maximum encoded transaction length configured for this network, if
    /// any. This property is not spec'ed out but can be inferred by looking how much data can be
    /// packed into a transaction for any given transaction type.
    fn max_encoded_tx_length(&self, ty: TxType) -> Option<usize>;

    /// Returns the strict maximum encoded transaction length for the given transaction type, if
    /// any.
    fn strict_max_encoded_tx_length(&self, ty: TxType) -> Option<usize>;

    /// Returns the reasonable minimum encoded transaction length, if any. This property is not
    /// spec'ed out but can be inferred by looking at which
    /// [`reth_primitives::PooledTransactionsElement`] will successfully pass decoding
    /// for any given transaction type.
    fn min_encoded_tx_length(&self, ty: TxType) -> Option<usize>;

    /// Returns the strict minimum encoded transaction length for the given transaction type, if
    /// any.
    fn strict_min_encoded_tx_length(&self, ty: TxType) -> Option<usize>;
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

/// Filters valid entries in [`NewPooledTransactionHashes68`] and [`NewPooledTransactionHashes66`]
/// in place, and flags misbehaving peers.
pub trait FilterAnnouncement {
    /// Removes invalid entries from a [`NewPooledTransactionHashes68`] announcement. Returns
    /// [`FilterOutcome::ReportPeer`] if the caller should penalize the peer, otherwise
    /// [`FilterOutcome::Ok`].
    fn filter_valid_entries_68(
        &self,
        msg: NewPooledTransactionHashes68,
    ) -> (FilterOutcome, ValidAnnouncementData)
    where
        Self: ValidateTx68;

    /// Removes invalid entries from a [`NewPooledTransactionHashes66`] announcement. Returns
    /// [`FilterOutcome::ReportPeer`] if the caller should penalize the peer, otherwise
    /// [`FilterOutcome::Ok`].
    fn filter_valid_entries_66(
        &self,
        msg: NewPooledTransactionHashes66,
    ) -> (FilterOutcome, ValidAnnouncementData);
}

/// Outcome from filtering [`NewPooledTransactionHashes68`]. Signals to caller whether to penalize
/// the sender of the announcement or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOutcome {
    /// Peer behaves appropriately.
    Ok,
    /// A penalty should be flagged for the peer. Peer sent an announcement with unacceptably
    /// invalid entries.
    ReportPeer,
}

/// Wrapper for types that implement [`FilterAnnouncement`]. The definition of a valid
/// announcement is network dependent. For example, different networks support different [`TxType`]
/// s, and different [`TxType`]s have different transaction size constraints. Defaults to
/// [`EthAnnouncementFilter`].
#[derive(Debug, Default, Deref, DerefMut)]
pub struct AnnouncementFilter<N = EthAnnouncementFilter>(N);

/// Filter for announcements containing EIP [`TxType`]s.
#[derive(Debug, Display, Default)]
pub struct EthAnnouncementFilter;

impl ValidateTx68 for EthAnnouncementFilter {
    fn should_fetch(&self, ty: u8, hash: TxHash, size: usize) -> ValidationOutcome {
        //
        // 1. checks if tx type is valid value for this network
        //
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

        //
        // 2. checks if tx's encoded length is within limits for this network
        //
        // transactions that are filtered out here, may not be spam, rather from benevolent peers
        // that are unknowingly sending announcements with invalid data.
        //
        if let Some(strict_min_encoded_tx_length) = self.strict_min_encoded_tx_length(tx_type) {
            if size < strict_min_encoded_tx_length {
                debug!(target: "net::eth-wire",
                    ty=ty,
                    size=size,
                    hash=%hash,
                    strict_min_encoded_tx_length=strict_min_encoded_tx_length,
                    network=%Self,
                    "invalid tx size in eth68 announcement"
                );

                return ValidationOutcome::Ignore
            }
        }
        if let Some(reasonable_min_encoded_tx_length) = self.min_encoded_tx_length(tx_type) {
            if size < reasonable_min_encoded_tx_length {
                debug!(target: "net::eth-wire",
                    ty=ty,
                    size=size,
                    hash=%hash,
                    reasonable_min_encoded_tx_length=reasonable_min_encoded_tx_length,
                    strict_min_encoded_tx_length=self.strict_min_encoded_tx_length(tx_type),
                    network=%Self,
                    "tx size in eth68 announcement, is unreasonably small"
                );

                // just log a tx which is smaller than the reasonable min encoded tx length, don't
                // filter it out
            }
        }
        // this network has no strict max encoded tx length for any tx type
        if let Some(reasonable_max_encoded_tx_length) = self.max_encoded_tx_length(tx_type) {
            if size > reasonable_max_encoded_tx_length {
                debug!(target: "net::eth-wire",
                    ty=ty,
                    size=size,
                    hash=%hash,
                    reasonable_max_encoded_tx_length=reasonable_max_encoded_tx_length,
                    strict_max_encoded_tx_length=self.strict_max_encoded_tx_length(tx_type),
                    network=%Self,
                    "tx size in eth68 announcement, is unreasonably large"
                );

                // just log a tx which is bigger than the reasonable max encoded tx length, don't
                // filter it out
            }
        }

        ValidationOutcome::Fetch
    }

    fn max_encoded_tx_length(&self, ty: TxType) -> Option<usize> {
        // the biggest transaction so far is a blob transaction, which is currently max 2^17,
        // encoded length, nonetheless, the blob tx may become bigger in the future.
        match ty {
            TxType::Legacy | TxType::EIP2930 | TxType::EIP1559 => Some(MAX_MESSAGE_SIZE),
            TxType::EIP4844 => None,
            #[cfg(feature = "optimism")]
            TxType::DEPOSIT => None,
        }
    }

    fn strict_max_encoded_tx_length(&self, _ty: TxType) -> Option<usize> {
        None
    }

    fn min_encoded_tx_length(&self, _ty: TxType) -> Option<usize> {
        // a transaction will have at least a signature. the encoded signature encoded on the tx
        // is at least as big as the decoded type.
        Some(SIGNATURE_DECODED_SIZE_BYTES)
    }

    fn strict_min_encoded_tx_length(&self, _ty: TxType) -> Option<usize> {
        // decoding a tx will exit right away if it's not at least a byte
        Some(1)
    }
}

impl FilterAnnouncement for EthAnnouncementFilter {
    fn filter_valid_entries_68(
        &self,
        msg: NewPooledTransactionHashes68,
    ) -> (FilterOutcome, ValidAnnouncementData)
    where
        Self: ValidateTx68,
    {
        trace!(target: "net::tx::validation",
            types=?msg.types,
            sizes=?msg.sizes,
            hashes=?msg.hashes,
            network=%Self,
            "validating eth68 announcement data.."
        );

        let NewPooledTransactionHashes68 { mut hashes, mut types, mut sizes } = msg;

        debug_assert!(
            hashes.len() == types.len() && hashes.len() == sizes.len(), "`%hashes`, `%types` and `%sizes` should all be the same length, decoding of `NewPooledTransactionHashes68` should handle this, 
`%hashes`: {hashes:?}, 
`%types`: {types:?}, 
`%sizes: {sizes:?}`"
        );

        //
        // 1. checks if the announcement is empty
        //
        if hashes.is_empty() {
            debug!(target: "net::tx",
                network=%Self,
                "empty eth68 announcement"
            );
            return (FilterOutcome::ReportPeer, HashMap::new())
        }

        let mut should_report_peer = false;
        let mut indices_to_remove = vec![];

        //
        // 2. checks if eth68 announcement metadata is valid
        //
        // transactions that are filtered out here, may not be spam, rather from benevolent peers
        // that are unknowingly sending announcements with invalid data.
        //
        for (i, (&ty, &hash, &size)) in izip!(&types, &hashes, &sizes).enumerate() {
            match self.should_fetch(ty, hash, size) {
                ValidationOutcome::Fetch => (),
                ValidationOutcome::Ignore => indices_to_remove.push(i),
                ValidationOutcome::ReportPeer => {
                    indices_to_remove.push(i);
                    should_report_peer = true;
                }
            }
        }

        for index in indices_to_remove.into_iter().rev() {
            hashes.remove(index);
            types.remove(index);
            sizes.remove(index);
        }

        //
        // 3. checks if announcement is spam packed with duplicate hashes
        //
        let mut deduped_data = HashMap::with_capacity(hashes.len());

        let original_len = hashes.len();

        for hash in hashes.into_iter().rev() {
            if let (Some(ty), Some(size)) = (types.pop(), sizes.pop()) {
                deduped_data.insert(hash, Some((ty, size)));
            }
        }
        if deduped_data.len() != original_len {
            should_report_peer = true
        }

        (
            if should_report_peer { FilterOutcome::ReportPeer } else { FilterOutcome::Ok },
            deduped_data,
        )
    }

    fn filter_valid_entries_66(
        &self,
        msg: NewPooledTransactionHashes66,
    ) -> (FilterOutcome, ValidAnnouncementData) {
        trace!(target: "net::tx::validation",
            hashes=?msg.0,
            network=%Self,
            "validating eth66 announcement data.."
        );

        let NewPooledTransactionHashes66(hashes) = msg;

        // 1. checks if the announcement is empty
        if hashes.is_empty() {
            debug!(target: "net::tx",
                network=%Self,
                "empty eth66 announcement"
            );
            return (FilterOutcome::ReportPeer, HashMap::new())
        }

        // 2. checks if announcement is spam packed with duplicate hashes
        let mut deduped_data = HashMap::with_capacity(hashes.len());

        let original_len = hashes.len();

        for hash in hashes.into_iter().rev() {
            deduped_data.insert(hash, None);
        }

        (
            if deduped_data.len() != original_len {
                FilterOutcome::ReportPeer
            } else {
                FilterOutcome::Ok
            },
            deduped_data,
        )
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use reth_eth_wire::MAX_MESSAGE_SIZE;
    use reth_primitives::B256;
    use std::str::FromStr;

    #[test]
    fn eth68_empty_announcement() {
        let types = vec![];
        let sizes = vec![];
        let hashes = vec![];

        let announcement = NewPooledTransactionHashes68 { types, sizes, hashes };

        let filter = EthAnnouncementFilter::default();

        let (outcome, _data) = filter.filter_valid_entries_68(announcement);

        assert_eq!(outcome, FilterOutcome::ReportPeer);
    }

    #[test]
    fn eth68_announcement_unrecognized_tx_type() {
        let types = vec![
            TxType::MAX_RESERVED_EIP as u8 + 1, // the first type isn't valid
            TxType::Legacy as u8,
        ];
        let sizes = vec![MAX_MESSAGE_SIZE, MAX_MESSAGE_SIZE];
        let hashes = vec![
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefbbbb")
                .unwrap(),
        ];

        let announcement = NewPooledTransactionHashes68 {
            types: types.clone(),
            sizes: sizes.clone(),
            hashes: hashes.clone(),
        };

        let filter = EthAnnouncementFilter::default();

        let (outcome, data) = filter.filter_valid_entries_68(announcement);

        assert_eq!(outcome, FilterOutcome::ReportPeer);

        let mut expected_data = HashMap::new();
        expected_data.insert(hashes[1], Some((types[1], sizes[1])));

        assert_eq!(expected_data, data,)
    }

    #[test]
    fn eth68_announcement_too_small_tx() {
        let types =
            vec![TxType::MAX_RESERVED_EIP as u8, TxType::Legacy as u8, TxType::EIP2930 as u8];
        let sizes = vec![
            0, // the first length isn't valid
            0, // neither is the second
            1,
        ];
        let hashes = vec![
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefbbbb")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeef00bb")
                .unwrap(),
        ];

        let announcement = NewPooledTransactionHashes68 {
            types: types.clone(),
            sizes: sizes.clone(),
            hashes: hashes.clone(),
        };

        let filter = EthAnnouncementFilter::default();

        let (outcome, data) = filter.filter_valid_entries_68(announcement);

        assert_eq!(outcome, FilterOutcome::Ok);

        let mut expected_data = HashMap::new();
        expected_data.insert(hashes[2], Some((types[2], sizes[2])));

        assert_eq!(expected_data, data,)
    }

    #[test]
    fn eth68_announcement_duplicate_tx_hash() {
        let types = vec![
            TxType::EIP1559 as u8,
            TxType::EIP4844 as u8,
            TxType::EIP1559 as u8,
            TxType::EIP4844 as u8,
        ];
        let sizes = vec![1, 1, 1, MAX_MESSAGE_SIZE];
        // first three or the same
        let hashes = vec![
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa") // dup
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa") // removed dup
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa") // removed dup
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefbbbb")
                .unwrap(),
        ];

        let announcement = NewPooledTransactionHashes68 {
            types: types.clone(),
            sizes: sizes.clone(),
            hashes: hashes.clone(),
        };

        let filter = EthAnnouncementFilter::default();

        let (outcome, data) = filter.filter_valid_entries_68(announcement);

        assert_eq!(outcome, FilterOutcome::ReportPeer);

        let mut expected_data = HashMap::new();
        expected_data.insert(hashes[3], Some((types[3], sizes[3])));
        expected_data.insert(hashes[0], Some((types[0], sizes[0])));

        assert_eq!(expected_data, data,)
    }

    #[test]
    fn eth66_empty_announcement() {
        let hashes = vec![];

        let announcement = NewPooledTransactionHashes66(hashes);

        let filter: AnnouncementFilter = AnnouncementFilter::default();

        let (outcome, _data) = filter.filter_valid_entries_66(announcement);

        assert_eq!(outcome, FilterOutcome::ReportPeer);
    }

    #[test]
    fn eth66_announcement_duplicate_tx_hash() {
        // first three or the same
        let hashes = vec![
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefbbbb") // dup1
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa") // dup2
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa") // removed dup2
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa") // removed dup2
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefbbbb") // removed dup1
                .unwrap(),
        ];

        let announcement = NewPooledTransactionHashes66(hashes.clone());

        let filter: AnnouncementFilter = AnnouncementFilter::default();

        let (outcome, data) = filter.filter_valid_entries_66(announcement);

        assert_eq!(outcome, FilterOutcome::ReportPeer);

        let mut expected_data = HashMap::new();
        expected_data.insert(hashes[1], None);
        expected_data.insert(hashes[0], None);

        assert_eq!(expected_data, data)
    }

    #[test]
    fn test_derive_more_display_for_zst() {
        let filter = EthAnnouncementFilter::default();
        assert_eq!("EthAnnouncementFilter", &filter.to_string());
    }
}
