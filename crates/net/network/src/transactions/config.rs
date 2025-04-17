use std::str::FromStr;

use super::{
    PeerMetadata, DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER,
    DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
    SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
};
use crate::transactions::constants::tx_fetcher::{
    DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH, DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS,
    DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER,
};
use derive_more::{Constructor, Display};
use reth_eth_wire::NetworkPrimitives;

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
#[derive(Debug, Clone, Default)]
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
    /// No restructions
    #[default]
    All,
    /// Propagate transactions to only trusted peers.
    Trusted,
}

impl TransactionPropagationPolicy for TransactionPropagationKind {
    fn can_propagate<N: NetworkPrimitives>(&self, peer: &mut PeerMetadata<N>) -> bool {
        match self {
            Self::All => true,
            Self::Trusted => peer.peer_kind.is_trusted(),
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
            _ => Err(format!("Invalid transaction propagation policy: {}", s)),
        }
    }
}
