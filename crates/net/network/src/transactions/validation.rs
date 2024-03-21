//! Validation of [`NewPooledTransactionHashes66`](reth_eth_wire::NewPooledTransactionHashes66)
//! and [`NewPooledTransactionHashes68`](reth_eth_wire::NewPooledTransactionHashes68)
//! announcements. Validation and filtering of announcements is network dependent.

use std::{fmt, fmt::Display, mem};

use crate::metrics::{AnnouncedTxTypesMetrics, TxTypesCounter};
use derive_more::{Deref, DerefMut};
use reth_eth_wire::{
    DedupPayload, Eth68TxMetadata, HandleMempoolData, PartiallyValidData, ValidAnnouncementData,
    MAX_MESSAGE_SIZE,
};
use reth_primitives::{Signature, TxHash, TxType};
use tracing::trace;

/// The size of a decoded signature in bytes.
pub const SIGNATURE_DECODED_SIZE_BYTES: usize = mem::size_of::<Signature>();

/// Interface for validating a `(ty, size, hash)` tuple from a
/// [`NewPooledTransactionHashes68`](reth_eth_wire::NewPooledTransactionHashes68)..
pub trait ValidateTx68 {
    /// Validates a [`NewPooledTransactionHashes68`](reth_eth_wire::NewPooledTransactionHashes68)
    /// entry. Returns [`ValidationOutcome`] which signals to the caller whether to fetch the
    /// transaction or wether to drop it, and whether the sender of the announcement should be
    /// penalized.
    fn should_fetch(
        &self,
        ty: u8,
        hash: &TxHash,
        size: usize,
        tx_types_counter: &mut TxTypesCounter,
    ) -> ValidationOutcome;

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

/// Outcomes from validating a `(ty, hash, size)` entry from a
/// [`NewPooledTransactionHashes68`](reth_eth_wire::NewPooledTransactionHashes68). Signals to the
/// caller how to deal with an announcement entry and the peer who sent the announcement.
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

/// Generic filter for announcements and responses. Checks for empty message and unique hashes/
/// transactions in message.
pub trait PartiallyFilterMessage {
    /// Removes duplicate entries from a mempool message. Returns [`FilterOutcome::ReportPeer`] if
    /// the caller should penalize the peer, otherwise [`FilterOutcome::Ok`].
    fn partially_filter_valid_entries<V>(
        &self,
        msg: impl DedupPayload<Value = V> + fmt::Debug,
    ) -> (FilterOutcome, PartiallyValidData<V>) {
        // 1. checks if the announcement is empty
        if msg.is_empty() {
            trace!(target: "net::tx",
                msg=?msg,
                "empty payload"
            );
            return (FilterOutcome::ReportPeer, PartiallyValidData::empty_eth66())
        }

        // 2. checks if announcement is spam packed with duplicate hashes
        let original_len = msg.len();
        let partially_valid_data = msg.dedup();

        (
            if partially_valid_data.len() != original_len {
                FilterOutcome::ReportPeer
            } else {
                FilterOutcome::Ok
            },
            partially_valid_data,
        )
    }
}

/// Filters valid entries in
/// [`NewPooledTransactionHashes68`](reth_eth_wire::NewPooledTransactionHashes68) and
/// [`NewPooledTransactionHashes66`](reth_eth_wire::NewPooledTransactionHashes66) in place, and
/// flags misbehaving peers.
pub trait FilterAnnouncement {
    /// Removes invalid entries from a
    /// [`NewPooledTransactionHashes68`](reth_eth_wire::NewPooledTransactionHashes68) announcement.
    /// Returns [`FilterOutcome::ReportPeer`] if the caller should penalize the peer, otherwise
    /// [`FilterOutcome::Ok`].
    fn filter_valid_entries_68(
        &self,
        msg: PartiallyValidData<Eth68TxMetadata>,
    ) -> (FilterOutcome, ValidAnnouncementData)
    where
        Self: ValidateTx68;

    /// Removes invalid entries from a
    /// [`NewPooledTransactionHashes66`](reth_eth_wire::NewPooledTransactionHashes66) announcement.
    /// Returns [`FilterOutcome::ReportPeer`] if the caller should penalize the peer, otherwise
    /// [`FilterOutcome::Ok`].
    fn filter_valid_entries_66(
        &self,
        msg: PartiallyValidData<Eth68TxMetadata>,
    ) -> (FilterOutcome, ValidAnnouncementData);
}

/// Outcome from filtering
/// [`NewPooledTransactionHashes68`](reth_eth_wire::NewPooledTransactionHashes68). Signals to caller
/// whether to penalize the sender of the announcement or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOutcome {
    /// Peer behaves appropriately.
    Ok,
    /// A penalty should be flagged for the peer. Peer sent an announcement with unacceptably
    /// invalid entries.
    ReportPeer,
}

/// Wrapper for types that implement [`FilterAnnouncement`]. The definition of a valid
/// announcement is network dependent. For example, different networks support different
/// [`TxType`]s, and different [`TxType`]s have different transaction size constraints. Defaults to
/// [`EthMessageFilter`].
#[derive(Debug, Default, Deref, DerefMut)]
pub struct MessageFilter<N = EthMessageFilter>(N);

/// Filter for announcements containing EIP [`TxType`]s.
#[derive(Debug, Default)]
pub struct EthMessageFilter {
    announced_tx_types_metrics: AnnouncedTxTypesMetrics,
}

impl Display for EthMessageFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EthMessageFilter")
    }
}

impl PartiallyFilterMessage for EthMessageFilter {}

impl ValidateTx68 for EthMessageFilter {
    fn should_fetch(
        &self,
        ty: u8,
        hash: &TxHash,
        size: usize,
        tx_types_counter: &mut TxTypesCounter,
    ) -> ValidationOutcome {
        //
        // 1. checks if tx type is valid value for this network
        //
        let tx_type = match TxType::try_from(ty) {
            Ok(ty) => ty,
            Err(_) => {
                trace!(target: "net::eth-wire",
                    ty=ty,
                    size=size,
                    hash=%hash,
                    network=%self,
                    "invalid tx type in eth68 announcement"
                );

                return ValidationOutcome::ReportPeer
            }
        };
        tx_types_counter.increase_by_tx_type(tx_type);

        //
        // 2. checks if tx's encoded length is within limits for this network
        //
        // transactions that are filtered out here, may not be spam, rather from benevolent peers
        // that are unknowingly sending announcements with invalid data.
        //
        if let Some(strict_min_encoded_tx_length) = self.strict_min_encoded_tx_length(tx_type) {
            if size < strict_min_encoded_tx_length {
                trace!(target: "net::eth-wire",
                    ty=ty,
                    size=size,
                    hash=%hash,
                    strict_min_encoded_tx_length=strict_min_encoded_tx_length,
                    network=%self,
                    "invalid tx size in eth68 announcement"
                );

                return ValidationOutcome::Ignore
            }
        }
        if let Some(reasonable_min_encoded_tx_length) = self.min_encoded_tx_length(tx_type) {
            if size < reasonable_min_encoded_tx_length {
                trace!(target: "net::eth-wire",
                    ty=ty,
                    size=size,
                    hash=%hash,
                    reasonable_min_encoded_tx_length=reasonable_min_encoded_tx_length,
                    strict_min_encoded_tx_length=self.strict_min_encoded_tx_length(tx_type),
                    network=%self,
                    "tx size in eth68 announcement, is unreasonably small"
                );

                // just log a tx which is smaller than the reasonable min encoded tx length, don't
                // filter it out
            }
        }
        // this network has no strict max encoded tx length for any tx type
        if let Some(reasonable_max_encoded_tx_length) = self.max_encoded_tx_length(tx_type) {
            if size > reasonable_max_encoded_tx_length {
                trace!(target: "net::eth-wire",
                    ty=ty,
                    size=size,
                    hash=%hash,
                    reasonable_max_encoded_tx_length=reasonable_max_encoded_tx_length,
                    strict_max_encoded_tx_length=self.strict_max_encoded_tx_length(tx_type),
                    network=%self,
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
        #[allow(unreachable_patterns)]
        match ty {
            TxType::Legacy | TxType::Eip2930 | TxType::Eip1559 => Some(MAX_MESSAGE_SIZE),
            TxType::Eip4844 => None,
            _ => None,
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

impl FilterAnnouncement for EthMessageFilter {
    fn filter_valid_entries_68(
        &self,
        mut msg: PartiallyValidData<Eth68TxMetadata>,
    ) -> (FilterOutcome, ValidAnnouncementData)
    where
        Self: ValidateTx68,
    {
        trace!(target: "net::tx::validation",
            msg=?*msg,
            network=%self,
            "validating eth68 announcement data.."
        );

        let mut should_report_peer = false;
        let mut tx_types_counter = TxTypesCounter::default();

        // checks if eth68 announcement metadata is valid
        //
        // transactions that are filtered out here, may not be spam, rather from benevolent peers
        // that are unknowingly sending announcements with invalid data.
        //
        msg.retain(|hash, metadata| {
            debug_assert!(
                metadata.is_some(),
                "metadata should exist for `%hash` in eth68 announcement passed to `%filter_valid_entries_68`,
`%hash`: {hash}"
            );

            let Some((ty, size)) = metadata else {
                return false
            };

            match self.should_fetch(*ty, hash, *size, &mut tx_types_counter) {
                ValidationOutcome::Fetch => true,
                ValidationOutcome::Ignore => false,
                ValidationOutcome::ReportPeer => {
                    should_report_peer = true;
                    false
                }
            }
        });
        self.announced_tx_types_metrics.update_eth68_announcement_metrics(tx_types_counter);
        (
            if should_report_peer { FilterOutcome::ReportPeer } else { FilterOutcome::Ok },
            ValidAnnouncementData::from_partially_valid_data(msg),
        )
    }

    fn filter_valid_entries_66(
        &self,
        partially_valid_data: PartiallyValidData<Option<(u8, usize)>>,
    ) -> (FilterOutcome, ValidAnnouncementData) {
        trace!(target: "net::tx::validation",
            hashes=?*partially_valid_data,
            network=%self,
            "validating eth66 announcement data.."
        );

        (FilterOutcome::Ok, ValidAnnouncementData::from_partially_valid_data(partially_valid_data))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use reth_eth_wire::{NewPooledTransactionHashes66, NewPooledTransactionHashes68};
    use reth_primitives::B256;
    use std::{collections::HashMap, str::FromStr};

    #[test]
    fn eth68_empty_announcement() {
        let types = vec![];
        let sizes = vec![];
        let hashes = vec![];

        let announcement = NewPooledTransactionHashes68 { types, sizes, hashes };

        let filter = EthMessageFilter::default();

        let (outcome, _partially_valid_data) = filter.partially_filter_valid_entries(announcement);

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

        let filter = EthMessageFilter::default();

        let (outcome, partially_valid_data) = filter.partially_filter_valid_entries(announcement);

        assert_eq!(outcome, FilterOutcome::Ok);

        let (outcome, valid_data) = filter.filter_valid_entries_68(partially_valid_data);

        assert_eq!(outcome, FilterOutcome::ReportPeer);

        let mut expected_data = HashMap::new();
        expected_data.insert(hashes[1], Some((types[1], sizes[1])));

        assert_eq!(expected_data, valid_data.into_data())
    }

    #[test]
    fn eth68_announcement_too_small_tx() {
        let types =
            vec![TxType::MAX_RESERVED_EIP as u8, TxType::Legacy as u8, TxType::Eip2930 as u8];
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

        let filter = EthMessageFilter::default();

        let (outcome, partially_valid_data) = filter.partially_filter_valid_entries(announcement);

        assert_eq!(outcome, FilterOutcome::Ok);

        let (outcome, valid_data) = filter.filter_valid_entries_68(partially_valid_data);

        assert_eq!(outcome, FilterOutcome::Ok);

        let mut expected_data = HashMap::new();
        expected_data.insert(hashes[2], Some((types[2], sizes[2])));

        assert_eq!(expected_data, valid_data.into_data())
    }

    #[test]
    fn eth68_announcement_duplicate_tx_hash() {
        let types = vec![
            TxType::Eip1559 as u8,
            TxType::Eip4844 as u8,
            TxType::Eip1559 as u8,
            TxType::Eip4844 as u8,
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

        let filter = EthMessageFilter::default();

        let (outcome, partially_valid_data) = filter.partially_filter_valid_entries(announcement);

        assert_eq!(outcome, FilterOutcome::ReportPeer);

        let mut expected_data = HashMap::new();
        expected_data.insert(hashes[3], Some((types[3], sizes[3])));
        expected_data.insert(hashes[0], Some((types[0], sizes[0])));

        assert_eq!(expected_data, partially_valid_data.into_data())
    }

    #[test]
    fn eth66_empty_announcement() {
        let hashes = vec![];

        let announcement = NewPooledTransactionHashes66(hashes);

        let filter: MessageFilter = MessageFilter::default();

        let (outcome, _partially_valid_data) = filter.partially_filter_valid_entries(announcement);

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

        let filter: MessageFilter = MessageFilter::default();

        let (outcome, partially_valid_data) = filter.partially_filter_valid_entries(announcement);

        assert_eq!(outcome, FilterOutcome::ReportPeer);

        let mut expected_data = HashMap::new();
        expected_data.insert(hashes[1], None);
        expected_data.insert(hashes[0], None);

        assert_eq!(expected_data, partially_valid_data.into_data())
    }

    #[test]
    fn test_display_for_zst() {
        let filter = EthMessageFilter::default();
        assert_eq!("EthMessageFilter", &filter.to_string());
    }
}
