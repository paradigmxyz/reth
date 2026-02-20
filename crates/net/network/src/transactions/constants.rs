/* ==================== BROADCAST ==================== */

/// Soft limit for the number of hashes in a
/// [`NewPooledTransactionHashes`](reth_eth_wire::NewPooledTransactionHashes) broadcast message.
///
/// Spec'd at 4096 hashes.
///
/// <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#newpooledtransactionhashes-0x08>
pub const SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE: usize = 4096;

/// Default soft limit for the byte size of a [`Transactions`](reth_eth_wire::Transactions)
/// broadcast message.
///
/// Default is 128 KiB.
pub const DEFAULT_SOFT_LIMIT_BYTE_SIZE_TRANSACTIONS_BROADCAST_MESSAGE: usize = 128 * 1024;

/// Maximum size of a single transaction that will be broadcast in full.
/// Transactions larger than this are only announced as hashes and must be
/// fetched by interested peers.
pub const DEFAULT_MAX_FULL_BROADCAST_TX_SIZE: usize = 4096;

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
    pub const DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER: u32 = 32_768;

    /// Default maximum pending pool imports to tolerate.
    ///
    /// Default is equivalent to the number of hashes in one full announcement, which is spec'd at
    /// 4096 hashes, so 4096 pending pool imports.
    pub const DEFAULT_MAX_COUNT_PENDING_POOL_IMPORTS: usize =
        SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE;
}

/// Constants used by [`TransactionFetcher`](super::TransactionFetcher).
pub mod tx_fetcher {
    use std::time::Duration;

    use reth_network_types::peers::config::{
        DEFAULT_MAX_COUNT_PEERS_INBOUND, DEFAULT_MAX_COUNT_PEERS_OUTBOUND,
    };

    use super::{
        SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
        SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST,
        SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE,
    };

    /// Slack added to timer checks to batch items expiring close together.
    pub const DEFAULT_TX_GATHER_SLACK: Duration = Duration::from_millis(100);

    /// Time to wait for a peer to respond before timing out.
    pub const DEFAULT_TX_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

    /// Multiplier applied to `tx_fetch_timeout` to determine how long a dangling request
    /// is kept before being force-removed. After this duration the peer's fetch slot is
    /// unblocked so it can receive new requests.
    pub const DANGLING_TIMEOUT_MULTIPLIER: u32 = 4;

    /// Max announcements tracked per peer across all stages.
    pub const DEFAULT_MAX_TX_ANNOUNCES_PER_PEER: usize = 4096;

    /* ============== SCALARS OF MESSAGES ============== */

    /// Default soft limit for the byte size of a
    /// [`PooledTransactions`](reth_eth_wire::PooledTransactions) response on assembling a
    /// [`GetPooledTransactions`](reth_eth_wire::PooledTransactions) request. This defaults to less
    /// than the [`SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE`], at 2 MiB, used when
    /// assembling a [`PooledTransactions`](reth_eth_wire::PooledTransactions) response.
    ///
    /// Default is 128 KiB.
    pub const DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ: usize = 128 * 1024;

    /* ==================== CONCURRENCY ==================== */

    /// Default maximum concurrent [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions)
    /// requests.
    ///
    /// Default is the sum of [`DEFAULT_MAX_COUNT_PEERS_INBOUND`] and
    /// [`DEFAULT_MAX_COUNT_PEERS_OUTBOUND`], which default to 30 and 100 peers respectively, so
    /// 130 requests.
    pub const DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS: u32 =
        DEFAULT_MAX_COUNT_PEERS_INBOUND + DEFAULT_MAX_COUNT_PEERS_OUTBOUND;

    /* =============== HASHES PENDING FETCH ================ */

    /// Default limit for number of transactions waiting for an idle peer to be fetched from.
    ///
    /// Default is 100 times the [`SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST`],
    /// which defaults to 256 hashes, so 25 600 hashes.
    pub const DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH: u32 =
        100 * SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST as u32;

    /* ================== ROUGH MEASURES ================== */

    /// Average byte size of an encoded transaction.
    ///
    /// Default is [`SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE`], which defaults to 2 MiB,
    /// divided by [`SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE`], which
    /// is spec'd at 4096 hashes, so 512 bytes.
    pub const AVERAGE_BYTE_SIZE_TX_ENCODED: usize =
        SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE /
            SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE;

    /* ================== REJECTION CACHES ================== */

    /// Default limit for number of bad imports to keep track of.
    ///
    /// Default is 100 KiB, i.e. 3 200 transaction hashes.
    pub const DEFAULT_MAX_COUNT_BAD_IMPORTS: u32 = 100 * 1024 / 32;

    /// Default limit for number of underpriced transaction hashes to cache.
    /// Transactions rejected as underpriced are cached to avoid re-fetching on subsequent
    /// announcements. Matches geth's `maxTxUnderpricedSetSize`.
    ///
    /// Default is 32 768 transaction hashes.
    pub const DEFAULT_MAX_COUNT_UNDERPRICED_IMPORTS: u32 = 32_768;
}
