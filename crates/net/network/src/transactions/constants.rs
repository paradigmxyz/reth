/* ==================== BROADCAST ==================== */

/// Soft limit for the number of hashes in a
/// [`NewPooledTransactionHashes`](reth_eth_wire::NewPooledTransactionHashes) broadcast message.
/// Spec'd at 4096 hashes.
///
/// <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#newpooledtransactionhashes-0x08>
pub const SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE: usize = 4096;

/// Default soft limit for the byte size of a [`Transactions`](reth_eth_wire::Transactions)
/// broadcast message. Default is 128 KiB.
pub const DEFAULT_SOFT_LIMIT_BYTE_SIZE_TRANSACTIONS_BROADCAST_MESSAGE: usize = 128 * 1024;

/* ================ REQUEST-RESPONSE ================ */

/// Recommended soft limit for the number of hashes in a
/// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request. Spec'd at 256 hashes
/// (8 KiB).
///
/// <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getpooledtransactions-0x09>
pub const SOFT_LIMIT_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST: usize = 256;

/// Soft limit for the byte size of a [`PooledTransactions`](reth_eth_wire::PooledTransactions)
/// response. This defaults to less than the standard maximum response size of 2 MiB (see specs).
/// Default is 128 KiB.
///
/// <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#protocol-messages>.
pub const DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE: usize = 128 * 1024;

pub mod tx_manager {
    use super::{
        tx_fetcher::DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS,
        SOFT_LIMIT_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST,
    };

    /// Default limit for number transactions to keep track of for a single peer, for transactions
    /// that the peer's pool and local pool have in common.
    pub const DEFAULT_CAPACITY_CACHE_SEEN_BY_PEER_AND_IN_POOL: usize = 10 * 1024;

    /// Default limit for the number of transactions to keep track of for a single peer, for
    /// transactions that are in the peer's pool but maybe not in the local pool yet.
    pub const DEFAULT_CAPACITY_CACHE_SENT_BY_PEER_AND_MAYBE_IN_POOL: usize = 10 * 1024;

    /// Default maximum pending pool imports to tolerate.
    pub const DEFAULT_MAX_COUNT_PENDING_POOL_IMPORTS: usize =
        SOFT_LIMIT_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST *
            DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS as usize;
}

pub mod tx_fetcher {
    use crate::peers::{DEFAULT_MAX_COUNT_PEERS_INBOUND, DEFAULT_MAX_COUNT_PEERS_OUTBOUND};

    use super::{
        DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
        SOFT_LIMIT_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST,
    };

    /* ==================== RETRIES ==================== */

    /// Default maximum request retires per [`TxHash`](reth_primitives::TxHash). Note, this is reset
    /// should the [`TxHash`](reth_primitives::TxHash) re-appear in an announcement after it has
    /// been evicted from the hashes pending fetch cache, i.e. the counter is restarted. If this
    /// happens, it is likely a very popular transaction, that should and can indeed be fetched
    /// hence this behaviour is favourable. Default is 2 retries.
    pub const DEFAULT_MAX_RETRIES: u8 = 2;

    /// Default number of alternative peers to keep track of for each transaction pending fetch. At
    /// most [`DEFAULT_MAX_RETRIES`], which defaults to 2 peers, can ever be needed per peer.
    /// Default is the sum of [`DEFAULT_MAX_RETRIES`] and
    /// [`DEFAULT_MARGINAL_COUNT_FALLBACK_PEERS`], which defaults to 1 peer, so 3 peers.
    pub const DEFAULT_MAX_COUNT_FALLBACK_PEERS: u8 =
        DEFAULT_MAX_RETRIES + DEFAULT_MARGINAL_COUNT_FALLBACK_PEERS;

    /// Default marginal on fallback peers. This is the case, since a transaction is only requested
    /// once from each individual peer. Default is 1 peer.
    const DEFAULT_MARGINAL_COUNT_FALLBACK_PEERS: u8 = 1;

    /* ==================== CONCURRENCY ==================== */

    /// Default maximum concurrent [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions)
    /// requests. Default is the product of [`DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER`],
    /// which defaults to 1 request, and the sum of [`DEFAULT_MAX_COUNT_PEERS_INBOUND`] and
    /// [`DEFAULT_MAX_COUNT_PEERS_OUTBOUND`], which default to 30 and 100 peers respectively, so 130
    /// requests.
    pub const DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS: u32 =
        DEFAULT_MAX_COUNT_PEERS_INBOUND + DEFAULT_MAX_COUNT_PEERS_OUTBOUND;

    /// Default maximum number of concurrent
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions)s to allow per peer. This
    /// number reflects concurrent requests for different hashes. Default is 1 request.
    pub const DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER: u8 = 1;

    /* =============== HASHES PENDING FETCH ================ */

    /// Default limit for number of transactions waiting for an idle peer to be fetched from.
    /// Default is 100 times the [`SOFT_LIMIT_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST`],
    /// which defaults to 256 hashes, so 25 600 hashes.
    pub const DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH: usize =
        100 * SOFT_LIMIT_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST;

    /// Default maximum number of hashes pending fetch to tolerate at any time. Default is half of
    /// [`DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH`], which defaults to 25 600 hashes, so 12 800
    /// hashes.
    pub const DEFAULT_MAX_COUNT_PENDING_FETCH: usize = DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH / 2;

    /* ====== LIMITED CAPACITY ON FETCH PENDING HASHES ====== */

    /// Default budget for finding an idle fallback peer for any hash pending fetch, when said
    /// search is budget constrained. Default is a sixth of [`DEFAULT_MAX_COUNT_PENDING_FETCH`],
    /// which defaults to 12 800 hashes (the breadth of the search), divided by
    /// [`DEFAULT_MAX_COUNT_FALLBACK_PEERS`], which defaults to 3 peers (the depth of the search),
    /// so the 711 lru hashes in the pending hashes cache.
    pub const DEFAULT_BUDGET_FIND_IDLE_FALLBACK_PEER: usize =
        DEFAULT_MAX_COUNT_PENDING_FETCH / 6 / DEFAULT_MAX_COUNT_FALLBACK_PEERS as usize;

    /// Default budget for finding hashes in the intersection of transactions announced by a peer
    /// and in the cache of hashes pending fetch, when said search is budget constrained. Default is
    /// a sixth of [`DEFAULT_MAX_COUNT_PENDING_FETCH`], which defaults to 12 800 hashes (the
    /// breadth of the search), so 2133 lru hashes in the pending hashes cache.
    pub const DEFAULT_BUDGET_FIND_INTERSECTION_ANNOUNCED_BY_PEER_AND_PENDING_FETCH: usize =
        DEFAULT_MAX_COUNT_PENDING_FETCH / 6;

    /* ====== SCALARS FOR USE ON FETCH PENDING HASHES ====== */

    /// Default max inflight request when fetching pending hashes. Default is half of
    /// [`DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS`], which defaults to 130 requests, so 65 requests.
    pub const DEFAULT_MAX_INFLIGHT_REQUESTS_ON_FETCH_PENDING_HASHES: usize =
        DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS as usize / 2;

    /// Default soft limit for the number of hashes in a
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request, when it is filled
    /// from hashes pending fetch. Default is half of the
    /// [`SOFT_LIMIT_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST`] which by spec is 256 hashes, so
    /// 128 hashes.
    pub const DEFAULT_SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST_ON_FETCH_PENDING_HASHES:
    usize = SOFT_LIMIT_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST / 2;

    /// Default soft limit for a [`PooledTransactions`](reth_eth_wire::PooledTransactions) response
    /// when it's used as expected response in calibrating the filling of a
    /// [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request, when the request is
    /// filled from hashes pending fetch. Default is half of
    /// [`DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE`], which defaults to 256 KiB, so
    /// 128 KiB.
    pub const DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE_ON_FETCH_PENDING_HASHES:
        usize = DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE / 2;

    /* ================== ROUGH MEASURES ================== */

    /// Average byte size of an encoded transaction. Default is
    /// [`DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE`], which defaults to 128 KiB,
    /// divided by [`SOFT_LIMIT_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST`], which defaults to
    /// 256 hashes, so 521 bytes.
    pub const AVERAGE_BYTE_SIZE_TX_ENCODED: usize =
        DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE /
            SOFT_LIMIT_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST;

    /// Median observed size in bytes of a small encoded legacy transaction. Default is 120 bytes.
    pub const MEDIAN_BYTE_SIZE_SMALL_LEGACY_TX_ENCODED: usize = 120;
}
