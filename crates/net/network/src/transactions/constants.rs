/* ==================== BROADCAST ==================== */

pub use reth_eth_wire_types::SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE;

/// Default soft limit for the byte size of a [`Transactions`](reth_eth_wire::Transactions)
/// broadcast message.
///
/// Default is 128 KiB.
pub const DEFAULT_SOFT_LIMIT_BYTE_SIZE_TRANSACTIONS_BROADCAST_MESSAGE: usize = 128 * 1024;

/* ================ REQUEST-RESPONSE ================ */

/// Recommended soft limit for the number of hashes in a
/// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request.
///
/// Spec'd at 256 hashes (8 KiB).
///
/// <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getpooledtransactions-0x09>
pub const SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST: usize = 256;

/// Soft limit for the byte size of a [`PooledTransactions`](reth_eth_wire::PooledTransactions)
/// response on assembling a [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions)
/// request.
///
/// Spec'd at 2 MiB.
///
/// <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#protocol-messages>.
pub const SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE: usize = 2 * 1024 * 1024;

/// Constants used by [`TransactionsManager`](super::TransactionsManager).
pub mod tx_manager {
    use super::SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE;

    /// Default limit for number of transactions to keep track of for a single peer.
    ///
    /// Default is 320 transaction hashes.
    pub const DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER: u32 = 10 * 1024 / 32;

    /// Default maximum pending pool imports to tolerate.
    ///
    /// Default is equivalent to the number of hashes in one full announcement, which is spec'd at
    /// 4096 hashes, so 4096 pending pool imports.
    pub const DEFAULT_MAX_COUNT_PENDING_POOL_IMPORTS: usize =
        SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE;

    /// Default limit for number of bad imports to keep track of.
    ///
    /// Default is 100 KiB, i.e. 3 200 transaction hashes.
    pub const DEFAULT_MAX_COUNT_BAD_IMPORTS: u32 = 100 * 1024 / 32;

    /// Default memory limit (in bytes) for the channel between
    /// [`NetworkManager`](crate::NetworkManager) and
    /// [`TransactionsManager`](crate::transactions::TransactionsManager).
    ///
    /// Caps the total in-flight bytes of `NetworkTransactionEvent`s buffered between the two
    /// tasks. When the budget is exhausted, new events are dropped (see metric
    /// `total_dropped_tx_events_at_full_capacity`).
    pub const DEFAULT_TX_MANAGER_CHANNEL_MEMORY_LIMIT_BYTES: usize = 1024 * 1024 * 1024;
}

/// Constants used by [`TransactionFetcher`](super::TransactionFetcher).
pub mod tx_fetcher {
    use super::{
        SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
        SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST,
        SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE,
    };
    use reth_network_types::peers::config::{
        DEFAULT_MAX_COUNT_PEERS_INBOUND, DEFAULT_MAX_COUNT_PEERS_OUTBOUND,
    };

    /// Default soft limit for the byte size of the expected
    /// [`PooledTransactions`](reth_eth_wire::PooledTransactions) response when packing a
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request. This is much
    /// smaller than the 2 MiB [`SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE`] applied when
    /// assembling a response, so that a single request fetches at most about one blob transaction
    /// and doesn't hog the connection to a peer.
    ///
    /// Default is 128 KiB.
    pub const DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ: usize = 128 * 1024;

    /// Default maximum number of concurrent
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) requests.
    ///
    /// Default is the sum of [`DEFAULT_MAX_COUNT_PEERS_INBOUND`] and
    /// [`DEFAULT_MAX_COUNT_PEERS_OUTBOUND`], which default to 30 and 100 peers respectively, so
    /// 130 requests.
    pub const DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS: u32 =
        DEFAULT_MAX_COUNT_PEERS_INBOUND + DEFAULT_MAX_COUNT_PEERS_OUTBOUND;

    /// Default maximum number of concurrent
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) requests per peer.
    ///
    /// Default is 1 request. With a single request at a time, the hashes a peer failed to deliver
    /// are known as soon as its response arrives and can be rescheduled immediately.
    pub const DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER: u8 = 1;

    /// Default maximum number of announced transaction hashes to keep track of, pending and
    /// inflight. Once reached, the oldest pending hash is evicted for a newly announced one.
    ///
    /// Default is 100 times the [`SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST`],
    /// which is 256 hashes, so 25 600 hashes.
    pub const DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH: u32 =
        100 * SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST as u32;

    /// Default maximum number of tracked hashes a single peer can be a candidate for. Bounds the
    /// memory a single peer can occupy in the fetcher with its announcements.
    ///
    /// Default is one full announcement,
    /// [`SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE`], so 4096 hashes.
    pub const DEFAULT_MAX_COUNT_ANNOUNCED_HASHES_PER_PEER: u32 =
        SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE as u32;

    /// Maximum number of peers remembered as candidates for a single hash, in the order they
    /// announced it. Announcements from further peers are ignored. This also bounds the number of
    /// peers a hash is requested from before it is given up on.
    ///
    /// Default is 16 peers.
    pub const MAX_COUNT_CANDIDATE_PEERS_PER_HASH: usize = 16;

    /// Number of candidates a hash is queued for right away. The remaining candidates only get
    /// the hash queued once one of these failed to deliver it, which keeps late announcements
    /// cheap while a hash is not given up on when its first announcers fail.
    ///
    /// Default is half of [`MAX_COUNT_CANDIDATE_PEERS_PER_HASH`], so 8 peers.
    pub const MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH: usize =
        MAX_COUNT_CANDIDATE_PEERS_PER_HASH / 2;

    /// Minimum number of hashes in a
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request while the number of
    /// hashes the pool can import is used up by inflight requests. Instead of stopping, requests
    /// shrink to this size, so that peers that don't respond can't stop fetching from the others.
    ///
    /// This is kept small because responses to these requests may exceed what the pool can
    /// import at once.
    ///
    /// Default is a sixteenth of [`SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST`],
    /// which is spec'd at 256 hashes, so 16 hashes.
    pub const MIN_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST: usize =
        SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST / 16;

    /// Assumed byte size of a transaction whose size wasn't announced, used when packing
    /// requests.
    ///
    /// Default is [`SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE`], which defaults to 2 MiB,
    /// divided by [`SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE`], which
    /// is spec'd at 4096 hashes, so 512 bytes.
    pub const AVERAGE_BYTE_SIZE_TX_ENCODED: usize =
        SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE /
            SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE;
}
