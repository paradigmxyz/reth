use std::{fmt::Debug, marker::PhantomData, str::FromStr};

use super::{
    PeerMetadata, DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER,
    DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
    SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
};
use crate::transactions::constants::tx_fetcher::{
    DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH, DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS,
    DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER,
};
use alloy_primitives::B256;
use derive_more::{Constructor, Display};

use reth_eth_wire::NetworkPrimitives;
use reth_ethereum_primitives::TxType;

/// Configuration for managing transactions within the network.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransactionsManagerConfig {
    /// Configuration for fetching transactions.
    pub transaction_fetcher_config: TransactionFetcherConfig,
    /// Max number of seen transactions to store for each peer.
    pub max_transactions_seen_by_peer_history: u32,
    /// How new pending transactions are propagated.
    #[cfg_attr(feature = "serde", serde(default))]
    pub propagation_mode: TransactionPropagationMode,
}

impl Default for TransactionsManagerConfig {
    fn default() -> Self {
        Self {
            transaction_fetcher_config: TransactionFetcherConfig::default(),
            max_transactions_seen_by_peer_history: DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER,
            propagation_mode: TransactionPropagationMode::default(),
        }
    }
}

/// Determines how new pending transactions are propagated to other peers in full.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TransactionPropagationMode {
    /// Send full transactions to sqrt of current peers.
    #[default]
    Sqrt,
    /// Always send transactions in full.
    All,
    /// Send full transactions to a maximum number of peers
    Max(usize),
}

impl TransactionPropagationMode {
    /// Returns the number of peers full transactions should be propagated to.
    pub(crate) fn full_peer_count(&self, peer_count: usize) -> usize {
        match self {
            Self::Sqrt => (peer_count as f64).sqrt().round() as usize,
            Self::All => peer_count,
            Self::Max(max) => peer_count.min(*max),
        }
    }
}
impl FromStr for TransactionPropagationMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_lowercase();
        match s.as_str() {
            "sqrt" => Ok(Self::Sqrt),
            "all" => Ok(Self::All),
            s => {
                if let Some(num) = s.strip_prefix("max:") {
                    num.parse::<usize>()
                        .map(TransactionPropagationMode::Max)
                        .map_err(|_| format!("Invalid number for Max variant: {num}"))
                } else {
                    Err(format!("Invalid transaction propagation mode: {s}"))
                }
            }
        }
    }
}

/// Configuration for fetching transactions.
#[derive(Debug, Constructor, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransactionFetcherConfig {
    /// Max inflight [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) requests.
    pub max_inflight_requests: u32,
    /// Max inflight [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) requests per
    /// peer.
    pub max_inflight_requests_per_peer: u8,
    /// Soft limit for the byte size of a
    /// [`PooledTransactions`](reth_eth_wire::PooledTransactions) response on assembling a
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request. Spec'd at 2
    /// MiB.
    pub soft_limit_byte_size_pooled_transactions_response: usize,
    /// Soft limit for the byte size of the expected
    /// [`PooledTransactions`](reth_eth_wire::PooledTransactions) response on packing a
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request with hashes.
    pub soft_limit_byte_size_pooled_transactions_response_on_pack_request: usize,
    /// Max capacity of the cache of transaction hashes, for transactions that weren't yet fetched.
    /// A transaction is pending fetch if its hash didn't fit into a
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) yet, or it wasn't returned
    /// upon request to peers.
    pub max_capacity_cache_txns_pending_fetch: u32,
}

impl Default for TransactionFetcherConfig {
    fn default() -> Self {
        Self {
            max_inflight_requests: DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS,
            max_inflight_requests_per_peer: DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER,
            soft_limit_byte_size_pooled_transactions_response:
                SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request:
                DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
                max_capacity_cache_txns_pending_fetch: DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH,
        }
    }
}

/// A policy defining which peers pending transactions are gossiped to.
pub trait TransactionPropagationPolicy: Send + Sync + Unpin + 'static {
    /// Filter a given peer based on the policy.
    ///
    /// This determines whether transactions can be propagated to this peer.
    fn can_propagate<N: NetworkPrimitives>(&self, peer: &mut PeerMetadata<N>) -> bool;

    /// A callback on the policy when a new peer session is established.
    fn on_session_established<N: NetworkPrimitives>(&mut self, peer: &mut PeerMetadata<N>);

    /// A callback on the policy when a peer session is closed.
    fn on_session_closed<N: NetworkPrimitives>(&mut self, peer: &mut PeerMetadata<N>);
}

/// Determines which peers pending transactions are propagated to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Display)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TransactionPropagationKind {
    /// Propagate transactions to all peers.
    ///
    /// No restrictions
    #[default]
    All,
    /// Propagate transactions to only trusted peers.
    Trusted,
    /// Do not propagate transactions
    None,
}

impl TransactionPropagationPolicy for TransactionPropagationKind {
    fn can_propagate<N: NetworkPrimitives>(&self, peer: &mut PeerMetadata<N>) -> bool {
        match self {
            Self::All => true,
            Self::Trusted => peer.peer_kind.is_trusted(),
            Self::None => false,
        }
    }

    fn on_session_established<N: NetworkPrimitives>(&mut self, _peer: &mut PeerMetadata<N>) {}

    fn on_session_closed<N: NetworkPrimitives>(&mut self, _peer: &mut PeerMetadata<N>) {}
}

impl FromStr for TransactionPropagationKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "All" | "all" => Ok(Self::All),
            "Trusted" | "trusted" => Ok(Self::Trusted),
            "None" | "none" => Ok(Self::None),
            _ => Err(format!("Invalid transaction propagation policy: {s}")),
        }
    }
}

/// Defines the outcome of evaluating a transaction against an `AnnouncementFilteringPolicy`.
///
/// Dictates how the `TransactionManager` should proceed on an announced transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnouncementAcceptance {
    /// Accept the transaction announcement.
    Accept,
    /// Log the transaction but not fetching the transaction or penalizing the peer.
    Ignore,
    /// Reject
    Reject {
        /// If true, the peer sending this announcement should be penalized.
        penalize_peer: bool,
    },
}

/// A policy that defines how to handle incoming transaction announcements,
/// particularly concerning transaction types and other announcement metadata.
pub trait AnnouncementFilteringPolicy: Send + Sync + Unpin + 'static {
    /// Decides how to handle a transaction announcement based on its type, hash, and size.
    fn decide_on_announcement(&self, ty: u8, hash: &B256, size: usize) -> AnnouncementAcceptance;
}

/// A generic `AnnouncementFilteringPolicy` that enforces strict validation
/// of transaction type based on a generic type `T`.
#[derive(Debug, Clone)]
pub struct TypedStrictFilter<T: TryFrom<u8> + Debug + Send + Sync + 'static>(PhantomData<T>);

impl<T: TryFrom<u8> + Debug + Send + Sync + 'static> Default for TypedStrictFilter<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T> AnnouncementFilteringPolicy for TypedStrictFilter<T>
where
    T: TryFrom<u8> + Debug + Send + Sync + Unpin + 'static,
    <T as TryFrom<u8>>::Error: Debug,
{
    fn decide_on_announcement(&self, ty: u8, hash: &B256, size: usize) -> AnnouncementAcceptance {
        match T::try_from(ty) {
            Ok(_valid_type) => AnnouncementAcceptance::Accept,
            Err(e) => {
                tracing::trace!(target: "net::tx::policy::strict_typed",
                    type_param = %std::any::type_name::<T>(),
                    %ty,
                    %size,
                    %hash,
                    error = ?e,
                    "Invalid or unrecognized transaction type byte. Rejecting entry and recommending peer penalization."
                );
                AnnouncementAcceptance::Reject { penalize_peer: true }
            }
        }
    }
}

/// Type alias for a `TypedStrictFilter`. This is the default strict announcement filter.
pub type StrictEthAnnouncementFilter = TypedStrictFilter<TxType>;

/// An [`AnnouncementFilteringPolicy`] that permissively handles unknown type bytes
/// based on a given type `T` using `T::try_from(u8)`.
///
/// If `T::try_from(ty)` succeeds, the announcement is accepted. Otherwise, it's ignored.
#[derive(Debug, Clone)]
pub struct TypedRelaxedFilter<T: TryFrom<u8> + Debug + Send + Sync + 'static>(PhantomData<T>);

impl<T: TryFrom<u8> + Debug + Send + Sync + 'static> Default for TypedRelaxedFilter<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T> AnnouncementFilteringPolicy for TypedRelaxedFilter<T>
where
    T: TryFrom<u8> + Debug + Send + Sync + Unpin + 'static,
    <T as TryFrom<u8>>::Error: Debug,
{
    fn decide_on_announcement(&self, ty: u8, hash: &B256, size: usize) -> AnnouncementAcceptance {
        match T::try_from(ty) {
            Ok(_valid_type) => AnnouncementAcceptance::Accept,
            Err(e) => {
                tracing::trace!(target: "net::tx::policy::relaxed_typed",
                    type_param = %std::any::type_name::<T>(),
                    %ty,
                    %size,
                    %hash,
                    error = ?e,
                    "Unknown transaction type byte. Ignoring entry."
                );
                AnnouncementAcceptance::Ignore
            }
        }
    }
}

/// Type alias for `TypedRelaxedFilter`. This filter accepts known Ethereum transaction types and
/// ignores unknown ones without penalizing the peer.
pub type RelaxedEthAnnouncementFilter = TypedRelaxedFilter<TxType>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_propagation_mode_from_str() {
        // Test "sqrt" variant
        assert_eq!(
            TransactionPropagationMode::from_str("sqrt").unwrap(),
            TransactionPropagationMode::Sqrt
        );
        assert_eq!(
            TransactionPropagationMode::from_str("SQRT").unwrap(),
            TransactionPropagationMode::Sqrt
        );
        assert_eq!(
            TransactionPropagationMode::from_str("Sqrt").unwrap(),
            TransactionPropagationMode::Sqrt
        );

        // Test "all" variant
        assert_eq!(
            TransactionPropagationMode::from_str("all").unwrap(),
            TransactionPropagationMode::All
        );
        assert_eq!(
            TransactionPropagationMode::from_str("ALL").unwrap(),
            TransactionPropagationMode::All
        );
        assert_eq!(
            TransactionPropagationMode::from_str("All").unwrap(),
            TransactionPropagationMode::All
        );

        // Test "max:N" variant
        assert_eq!(
            TransactionPropagationMode::from_str("max:10").unwrap(),
            TransactionPropagationMode::Max(10)
        );
        assert_eq!(
            TransactionPropagationMode::from_str("MAX:42").unwrap(),
            TransactionPropagationMode::Max(42)
        );
        assert_eq!(
            TransactionPropagationMode::from_str("Max:100").unwrap(),
            TransactionPropagationMode::Max(100)
        );

        // Test invalid inputs
        assert!(TransactionPropagationMode::from_str("invalid").is_err());
        assert!(TransactionPropagationMode::from_str("max:not_a_number").is_err());
        assert!(TransactionPropagationMode::from_str("max:").is_err());
        assert!(TransactionPropagationMode::from_str("max").is_err());
        assert!(TransactionPropagationMode::from_str("").is_err());
    }
}
