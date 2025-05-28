//! Validation of [`NewPooledTransactionHashes66`](reth_eth_wire::NewPooledTransactionHashes66)
//! and [`NewPooledTransactionHashes68`](reth_eth_wire::NewPooledTransactionHashes68)
//! announcements. Validation and filtering of announcements is network dependent.

use alloy_primitives::Signature;
use derive_more::{Deref, DerefMut};
use reth_eth_wire::{DedupPayload, HandleMempoolData, PartiallyValidData};
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

/// A generic wrapper for types that provide message filtering capabilities.
///
/// This struct is typically used with types implementing traits like [`PartiallyFilterMessage`],
/// which perform initial stateless validation on network messages, such as checking for empty
/// payloads or removing duplicate entries.
#[derive(Debug, Default, Deref, DerefMut)]
pub struct MessageFilter<N = EthMessageFilter>(N);

/// Filter for announcements containing EIP [`reth_ethereum_primitives::TxType`]s.
#[derive(Debug, Default)]
pub struct EthMessageFilter;

impl Display for EthMessageFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EthMessageFilter")
    }
}

impl PartiallyFilterMessage for EthMessageFilter {}

#[cfg(test)]
mod test {
    use super::*;
    use alloy_primitives::B256;
    use reth_eth_wire::{NewPooledTransactionHashes66, NewPooledTransactionHashes68};
    use std::{collections::HashMap, str::FromStr};

    #[test]
    fn eth68_empty_announcement() {
        let types = vec![];
        let sizes = vec![];
        let hashes = vec![];

        let announcement = NewPooledTransactionHashes68 { types, sizes, hashes };

        let filter = EthMessageFilter;

        let (outcome, _partially_valid_data) = filter.partially_filter_valid_entries(announcement);

        assert_eq!(outcome, FilterOutcome::ReportPeer);
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
    fn test_display_for_zst() {
        let filter = EthMessageFilter;
        assert_eq!("EthMessageFilter", &filter.to_string());
    }
}
