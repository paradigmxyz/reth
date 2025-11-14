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
}

/// Constants used by [`TransactionFetcher`](super::TransactionFetcher).
pub mod tx_fetcher {
    use reth_network_types::peers::config::{
        DEFAULT_MAX_COUNT_PEERS_INBOUND, DEFAULT_MAX_COUNT_PEERS_OUTBOUND,
    };

    use super::{
        SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
        SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST,
        SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE,
    };

    /* ============== SCALARS OF MESSAGES ============== */

    /// Default soft limit for the byte size of a
    /// [`PooledTransactions`](reth_eth_wire::PooledTransactions) response on assembling a
    /// [`GetPooledTransactions`](reth_eth_wire::PooledTransactions) request. This defaults to less
    /// than the [`SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE`], at 2 MiB, used when
    /// assembling a [`PooledTransactions`](reth_eth_wire::PooledTransactions) response.
    ///
    /// Default is 128 KiB.
    pub const DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ: usize = 128 * 1024;

    /* ==================== RETRIES ==================== */

    /// Default maximum request retires per [`TxHash`](alloy_primitives::TxHash). Note, this is
    /// reset should the [`TxHash`](alloy_primitives::TxHash) re-appear in an announcement after it
    /// has been evicted from the hashes pending fetch cache, i.e. the counter is restarted. If
    /// this happens, it is likely a very popular transaction, that should and can indeed be
    /// fetched hence this behaviour is favourable.
    ///
    /// Default is 2 retries.
    pub const DEFAULT_MAX_RETRIES: u8 = 2;

    /// Default number of alternative peers to keep track of for each transaction pending fetch. At
    /// most [`DEFAULT_MAX_RETRIES`], which defaults to 2 peers, can ever be needed per peer.
    ///
    /// Default is the sum of [`DEFAULT_MAX_RETRIES`] an
    /// [`DEFAULT_MARGINAL_COUNT_FALLBACK_PEERS`], which defaults to 1 peer, so 3 peers.
    pub const DEFAULT_MAX_COUNT_FALLBACK_PEERS: u8 =
        DEFAULT_MAX_RETRIES + DEFAULT_MARGINAL_COUNT_FALLBACK_PEERS;

    /// Default marginal on fallback peers. This is the case, since a transaction is only requested
    /// once from each individual peer.
    ///
    /// Default is 1 peer.
    pub const DEFAULT_MARGINAL_COUNT_FALLBACK_PEERS: u8 = 1;

    /* ==================== CONCURRENCY ==================== */

    /// Default maximum concurrent [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions)
    /// requests.
    ///
    /// Default is the product of [`DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER`], which
    /// defaults to 1 request, and the sum of [`DEFAULT_MAX_COUNT_PEERS_INBOUND`] and
    /// [`DEFAULT_MAX_COUNT_PEERS_OUTBOUND`], which default to 30 and 100 peers respectively, so
    /// 130 requests.
    pub const DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS: u32 =
        DEFAULT_MAX_COUNT_PEERS_INBOUND + DEFAULT_MAX_COUNT_PEERS_OUTBOUND;

    /// Default maximum number of concurrent
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions)s to allow per peer. This
    /// number reflects concurrent requests for different hashes.
    ///
    /// Default is 1 request.
    pub const DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER: u8 = 1;

    /* =============== HASHES PENDING FETCH ================ */

    /// Default limit for number of transactions waiting for an idle peer to be fetched from.
    ///
    /// Default is 100 times the [`SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST`],
    /// which defaults to 256 hashes, so 25 600 hashes.
    pub const DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH: u32 =
        100 * SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST as u32;

    /// Default max size for cache of inflight and pending transactions fetch.
    ///
    /// Default is [`DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH`] +
    /// [`DEFAULT_MAX_COUNT_INFLIGHT_REQUESTS_ON_FETCH_PENDING_HASHES`], which is 25600 hashes and
    /// 65 requests, so it is 25665 hashes.
    pub const DEFAULT_MAX_CAPACITY_CACHE_INFLIGHT_AND_PENDING_FETCH: u32 =
        DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH +
            DEFAULT_MAX_COUNT_INFLIGHT_REQUESTS_ON_FETCH_PENDING_HASHES as u32;

    /// Default maximum number of hashes pending fetch to tolerate at any time.
    ///
    /// Default is half of [`DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH`], which defaults to 25 600
    /// hashes, so 12 800 hashes.
    pub const DEFAULT_MAX_COUNT_PENDING_FETCH: usize =
        DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH as usize / 2;

    /* ====== LIMITED CAPACITY ON FETCH PENDING HASHES ====== */

    /* ====== SCALARS FOR USE ON FETCH PENDING HASHES ====== */

    /// Default soft limit for the number of hashes in a
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request, when it is filled
    /// from hashes pending fetch.
    ///
    /// Default is half of the [`SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST`]
    /// which by spec is 256 hashes, so 128 hashes.
    pub const DEFAULT_SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST_ON_FETCH_PENDING_HASHES:
    usize = SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST / 2;

    /// Default soft limit for a [`PooledTransactions`](reth_eth_wire::PooledTransactions) response
    /// when it's used as expected response in calibrating the filling of a
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request, when the request
    /// is filled from hashes pending fetch.
    ///
    /// Default is half of
    /// [`DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ`],
    /// which defaults to 128 KiB, so 64 KiB.
    pub const DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE_ON_FETCH_PENDING_HASHES:
        usize =
        DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ /
            2;

    /// Default max inflight request when fetching pending hashes.
    ///
    /// Default is half of [`DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS`], which defaults to 130
    /// requests, so 65 requests.
    pub const DEFAULT_MAX_COUNT_INFLIGHT_REQUESTS_ON_FETCH_PENDING_HASHES: usize =
        DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS as usize / 2;

    /* ================== ROUGH MEASURES ================== */

    /// Average byte size of an encoded transaction.
    ///
    /// Default is [`SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE`], which defaults to 2 MiB,
    /// divided by [`SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE`], which
    /// is spec'd at 4096 hashes, so 521 bytes.
    pub const AVERAGE_BYTE_SIZE_TX_ENCODED: usize =
        SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE /
            SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE;

    /// Median observed size in bytes of a small encoded legacy transaction.
    ///
    /// Default is 120 bytes.
    pub const MEDIAN_BYTE_SIZE_SMALL_LEGACY_TX_ENCODED: usize = 120;

    /// Marginal on the number of hashes to preallocate memory for in a
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request, when packed
    /// according to the [`Eth68`](reth_eth_wire::EthVersion::Eth68) protocol version. To make
    /// sure enough memory is preallocated in most cases, it's sensible to use a margin. This,
    /// since the capacity is calculated based on median value
    /// [`MEDIAN_BYTE_SIZE_SMALL_LEGACY_TX_ENCODED`]. There may otherwise be a noteworthy number of
    /// cases where just 1 or 2 bytes too little memory is preallocated.
    ///
    /// Default is 8 hashes.
    pub const DEFAULT_MARGINAL_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST: usize = 8;
}
