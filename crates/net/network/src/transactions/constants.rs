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
    /// Default is 10 KiB.
    pub const DEFAULT_CAPACITY_CACHE_SEEN_BY_PEER: usize = 10 * 1024;

    /// Default maximum pending pool imports to tolerate.
    ///
    /// Default is equivalent to the number of hashes in one full announcement, which is spec'd at
    /// 4096 hashes, so 4096 pending pool imports.
    pub const DEFAULT_MAX_COUNT_PENDING_POOL_IMPORTS: usize =
        SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE;

    /// Default limit for number of bad imports to keep track of.
    ///
    /// Default is 10 KiB.
    pub const DEFAULT_CAPACITY_CACHE_BAD_IMPORTS: usize = 100 * 1024;
}

/// Constants used by [`TransactionFetcher`](super::TransactionFetcher).
pub mod tx_fetcher {
    use crate::{
        peers::{DEFAULT_MAX_COUNT_PEERS_INBOUND, DEFAULT_MAX_COUNT_PEERS_OUTBOUND},
        transactions::fetcher::TransactionFetcherInfo,
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

    /// Default maximum request retires per [`TxHash`](reth_primitives::TxHash). Note, this is
    /// reset should the [`TxHash`](reth_primitives::TxHash) re-appear in an announcement after it
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
    pub const DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH: usize =
        100 * SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST;

    /// Default max size for cache of inflight and pending transactions fetch.
    ///
    /// Default is [`DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH`] +
    /// [`DEFAULT_MAX_COUNT_INFLIGHT_REQUESTS_ON_FETCH_PENDING_HASHES`], which is 25600 hashes and
    /// 65 requests, so it is 25665 hashes.
    pub const DEFAULT_MAX_CAPACITY_CACHE_INFLIGHT_AND_PENDING_FETCH: usize =
        DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH +
            DEFAULT_MAX_COUNT_INFLIGHT_REQUESTS_ON_FETCH_PENDING_HASHES;

    /// Default maximum number of hashes pending fetch to tolerate at any time.
    ///
    /// Default is half of [`DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH`], which defaults to 25 600
    /// hashes, so 12 800 hashes.
    pub const DEFAULT_MAX_COUNT_PENDING_FETCH: usize = DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH / 2;

    /* ====== LIMITED CAPACITY ON FETCH PENDING HASHES ====== */

    /// Default budget for finding an idle fallback peer for any hash pending fetch, when said
    /// search is budget constrained.
    ///
    /// Default is a sixth of [`DEFAULT_MAX_COUNT_PENDING_FETCH`], which defaults to 12 800 hashes
    /// (the breadth of the search), divided by [`DEFAULT_MAX_COUNT_FALLBACK_PEERS`], which
    /// defaults to 3 peers (the depth of the search), so the 711 lru hashes in the pending hashes
    /// cache.
    pub const DEFAULT_BUDGET_FIND_IDLE_FALLBACK_PEER: usize =
        DEFAULT_MAX_COUNT_PENDING_FETCH / 6 / DEFAULT_MAX_COUNT_FALLBACK_PEERS as usize;

    /// Default budget for finding hashes in the intersection of transactions announced by a peer
    /// and in the cache of hashes pending fetch, when said search is budget constrained.
    ///
    /// Default is a sixth of [`DEFAULT_MAX_COUNT_PENDING_FETCH`], which defaults to 12 800 hashes
    /// (the breadth of the search), so 2133 lru hashes in the pending hashes cache.
    pub const DEFAULT_BUDGET_FIND_INTERSECTION_ANNOUNCED_BY_PEER_AND_PENDING_FETCH: usize =
        DEFAULT_MAX_COUNT_PENDING_FETCH / 6;

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

    /// Default divisor of the max inflight request when calculating search breadth of the search
    /// for any idle peer to which to send a request filled with hashes pending fetch. The max
    /// inflight requests is configured in [`TransactionFetcherInfo`].
    ///
    /// Default is 3 requests.
    pub const DEFAULT_DIVISOR_MAX_COUNT_INFLIGHT_REQUESTS_ON_FIND_IDLE_PEER: usize = 3;

    /// Default divisor of the max inflight request when calculating search breadth of the search
    /// for the intersection of hashes announced by a peer and hashes pending fetch. The max
    /// inflight requests is configured in [`TransactionFetcherInfo`].
    ///
    /// Default is 2 requests.
    pub const DEFAULT_DIVISOR_MAX_COUNT_INFLIGHT_REQUESTS_ON_FIND_INTERSECTION: usize = 2;

    // Default divisor to the max pending pool imports when calculating search breadth of the
    /// search for any idle peer to which to send a request filled with hashes pending fetch.
    /// The max pending pool imports is configured in
    /// [`PendingPoolImportsInfo`](crate::transactions::PendingPoolImportsInfo).
    ///
    /// Default is 4 requests.
    pub const DEFAULT_DIVISOR_MAX_COUNT_PENDING_POOL_IMPORTS_ON_FIND_IDLE_PEER: usize = 4;

    /// Default divisor to the max pending pool imports when calculating search breadth of the
    /// search for any idle peer to which to send a request filled with hashes pending fetch.
    /// The max pending pool imports is configured in
    /// [`PendingPoolImportsInfo`](crate::transactions::PendingPoolImportsInfo).
    ///
    /// Default is 3 requests.
    pub const DEFAULT_DIVISOR_MAX_COUNT_PENDING_POOL_IMPORTS_ON_FIND_INTERSECTION: usize = 3;

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

    /// Returns the approx number of transaction hashes that a
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request will have capacity
    /// for w.r.t. the [`Eth68`](reth_eth_wire::EthVersion::Eth68) protocol version. This is useful
    /// for preallocating memory.
    pub const fn approx_capacity_get_pooled_transactions_req_eth68(
        info: &TransactionFetcherInfo,
    ) -> usize {
        let max_size_expected_response =
            info.soft_limit_byte_size_pooled_transactions_response_on_pack_request;

        max_size_expected_response / MEDIAN_BYTE_SIZE_SMALL_LEGACY_TX_ENCODED +
            DEFAULT_MARGINAL_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST
    }

    /// Returns the approx number of transactions that a
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request will
    /// have capacity for w.r.t. the [`Eth66`](reth_eth_wire::EthVersion::Eth66) protocol version.
    /// This is useful for preallocating memory.
    pub const fn approx_capacity_get_pooled_transactions_req_eth66() -> usize {
        SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST
    }
}
