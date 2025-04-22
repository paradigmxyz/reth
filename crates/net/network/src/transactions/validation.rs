//! Validation of [`NewPooledTransactionHashes66`](reth_eth_wire::NewPooledTransactionHashes66)
//! and [`NewPooledTransactionHashes68`](reth_eth_wire::NewPooledTransactionHashes68)
//! announcements. Validation and filtering of announcements is network dependent.

use crate::metrics::{AnnouncedTxTypesMetrics, TxTypesCounter};
use alloy_primitives::Signature;
use derive_more::{Deref, DerefMut};
use reth_eth_wire::{
    DedupPayload, Eth68TxMetadata, HandleMempoolData, PartiallyValidData, ValidAnnouncementData,
};
use reth_ethereum_primitives::TxType;
use std::{fmt, fmt::Display, mem};
use tracing::trace;

/// The size of a decoded signature in bytes.
pub const SIGNATURE_DECODED_SIZE_BYTES: usize = mem::size_of::<Signature>();

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
            if partially_valid_data.len() == original_len {
                FilterOutcome::Ok
            } else {
                FilterOutcome::ReportPeer
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
    ) -> (FilterOutcome, ValidAnnouncementData);

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

impl FilterAnnouncement for EthMessageFilter {
    fn filter_valid_entries_68(
        &self,
        mut msg: PartiallyValidData<Eth68TxMetadata>,
    ) -> (FilterOutcome, ValidAnnouncementData) {
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

            //
            //  checks if tx type is valid value for this network
            //
            let tx_type = match TxType::try_from(*ty) {
                Ok(ty) => ty,
                Err(_) => {
                    trace!(target: "net::eth-wire",
                        ty=ty,
                        size=size,
                        hash=%hash,
                        network=%self,
                        "invalid tx type in eth68 announcement"
                    );

                    should_report_peer = true;
                    return false;
            }

            };
            tx_types_counter.increase_by_tx_type(tx_type);

            true
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
    use alloy_primitives::B256;
    use reth_eth_wire::{
        NewPooledTransactionHashes66, NewPooledTransactionHashes68, MAX_MESSAGE_SIZE,
    };
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
            TxType::Eip7702 as u8 + 1, // the first type isn't valid
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

        let mut expected_data = HashMap::default();
        expected_data.insert(hashes[1], Some((types[1], sizes[1])));

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

        let mut expected_data = HashMap::default();
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

        let mut expected_data = HashMap::default();
        expected_data.insert(hashes[1], None);
        expected_data.insert(hashes[0], None);

        assert_eq!(expected_data, partially_valid_data.into_data())
    }

    #[test]
    fn eth68_announcement_eip7702_tx() {
        let types = vec![TxType::Eip7702 as u8, TxType::Legacy as u8];
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
        assert_eq!(outcome, FilterOutcome::Ok);

        let mut expected_data = HashMap::default();
        expected_data.insert(hashes[0], Some((types[0], sizes[0])));
        expected_data.insert(hashes[1], Some((types[1], sizes[1])));

        assert_eq!(expected_data, valid_data.into_data());
    }

    #[test]
    fn eth68_announcement_eip7702_tx_size_validation() {
        let types = vec![TxType::Eip7702 as u8, TxType::Eip7702 as u8, TxType::Eip7702 as u8];
        // Test with different sizes: too small, reasonable, too large
        let sizes = vec![
            1,                    // too small
            MAX_MESSAGE_SIZE / 2, // reasonable size
            MAX_MESSAGE_SIZE + 1, // too large
        ];
        let hashes = vec![
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefbbbb")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcccc")
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

        let mut expected_data = HashMap::default();

        for i in 0..3 {
            expected_data.insert(hashes[i], Some((types[i], sizes[i])));
        }

        assert_eq!(expected_data, valid_data.into_data());
    }

    #[test]
    fn eth68_announcement_mixed_tx_types() {
        let types = vec![
            TxType::Legacy as u8,
            TxType::Eip7702 as u8,
            TxType::Eip1559 as u8,
            TxType::Eip4844 as u8,
        ];
        let sizes = vec![MAX_MESSAGE_SIZE, MAX_MESSAGE_SIZE, MAX_MESSAGE_SIZE, MAX_MESSAGE_SIZE];
        let hashes = vec![
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafa")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefbbbb")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcccc")
                .unwrap(),
            B256::from_str("0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefdddd")
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

        let mut expected_data = HashMap::default();
        // All transaction types should be included as they are valid
        for i in 0..4 {
            expected_data.insert(hashes[i], Some((types[i], sizes[i])));
        }

        assert_eq!(expected_data, valid_data.into_data());
    }

    #[test]
    fn test_display_for_zst() {
        let filter = EthMessageFilter::default();
        assert_eq!("EthMessageFilter", &filter.to_string());
    }
}
