use derive_more::Constructor;

use super::{
    DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER,
    DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
    SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
};
use crate::transactions::constants::tx_fetcher::{
    DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS, DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER,
};

/// Configuration for managing transactions within the network.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransactionsManagerConfig {
    /// Configuration for fetching transactions.
    pub transaction_fetcher_config: TransactionFetcherConfig,
    /// Max number of seen transactions to store for each peer.
    pub max_transactions_seen_by_peer_history: u32,
}

impl Default for TransactionsManagerConfig {
    fn default() -> Self {
        Self {
            transaction_fetcher_config: TransactionFetcherConfig::default(),
            max_transactions_seen_by_peer_history: DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER,
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
}

impl Default for TransactionFetcherConfig {
    fn default() -> Self {
        Self {
            max_inflight_requests: DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS,
            max_inflight_requests_per_peer: DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER,
            soft_limit_byte_size_pooled_transactions_response:
                SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request:
                DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ
        }
    }
}
