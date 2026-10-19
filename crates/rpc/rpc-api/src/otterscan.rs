use alloy_eips::{eip1898::LenientBlockNumberOrTag, BlockId};
use alloy_json_rpc::RpcObject;
use alloy_primitives::{Address, Bytes, TxHash, B256};
use alloy_rpc_types_trace::otterscan::{
    BlockDetails, ContractCreator, InternalOperation, OtsBlockTransactions, TraceEntry,
    TransactionsWithReceipts,
};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};

/// Otterscan RPC interface.
///
/// Reth implements a subset of the API. In particular, address history search is unimplemented
/// even though `getApiLevel` returns 8. Historical queries require the relevant state and history.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "ots"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "ots"))]
pub trait Otterscan<T: RpcObject, H: RpcObject> {
    /// Get the block header by block number, required by otterscan.
    /// Otterscan currently requires this endpoint, used as:
    ///
    /// 1. check if the node is Erigon or not
    /// 2. get block header instead of the full block
    ///
    /// Ref: <https://github.com/otterscan/otterscan/blob/071d8c55202badf01804f6f8d53ef9311d4a9e47/src/useProvider.ts#L71>
    #[method(name = "getHeaderByNumber", aliases = ["erigon_getHeaderByNumber"])]
    async fn get_header_by_number(
        &self,
        block_number: LenientBlockNumberOrTag,
    ) -> RpcResult<Option<H>>;

    /// Check if a certain address contains a deployed code.
    #[method(name = "hasCode")]
    async fn has_code(&self, address: Address, block_id: Option<BlockId>) -> RpcResult<bool>;

    /// Returns API level 8 for frontend compatibility, not a guarantee of complete support.
    /// In particular, both address history search methods remain unimplemented.
    #[method(name = "getApiLevel")]
    async fn get_api_level(&self) -> RpcResult<u64>;

    /// Return the internal ETH transfers inside a transaction.
    #[method(name = "getInternalOperations")]
    async fn get_internal_operations(&self, tx_hash: TxHash) -> RpcResult<Vec<InternalOperation>>;

    /// Given a transaction hash, returns its raw revert reason.
    /// Known transactions without revert data return empty bytes; unknown transactions return
    /// `None`.
    #[method(name = "getTransactionError")]
    async fn get_transaction_error(&self, tx_hash: TxHash) -> RpcResult<Option<Bytes>>;

    /// Extract all variations of calls, contract creation and self-destructs and returns a call
    /// tree.
    #[method(name = "traceTransaction")]
    async fn trace_transaction(&self, tx_hash: TxHash) -> RpcResult<Option<Vec<TraceEntry>>>;

    /// Tailor-made and expanded version of `eth_getBlockByNumber` for block details page in
    /// Otterscan.
    #[method(name = "getBlockDetails")]
    async fn get_block_details(
        &self,
        block_number: LenientBlockNumberOrTag,
    ) -> RpcResult<BlockDetails<H>>;

    /// Tailor-made and expanded version of `eth_getBlockByHash` for block details page in
    /// Otterscan.
    #[method(name = "getBlockDetailsByHash")]
    async fn get_block_details_by_hash(&self, block_hash: B256) -> RpcResult<BlockDetails<H>>;

    /// Get paginated transactions for a certain block. Also remove some verbose fields like logs.
    /// Page zero selects the block's last transactions; each page retains ascending block order.
    #[method(name = "getBlockTransactions")]
    async fn get_block_transactions(
        &self,
        block_number: LenientBlockNumberOrTag,
        page_number: usize,
        page_size: usize,
    ) -> RpcResult<OtsBlockTransactions<T, H>>;

    /// Gets paginated inbound/outbound transaction calls for a certain address.
    ///
    /// Unimplemented: returns JSON-RPC error -32603 with message "unimplemented".
    /// See <https://github.com/paradigmxyz/reth/issues/13499>.
    #[method(name = "searchTransactionsBefore")]
    async fn search_transactions_before(
        &self,
        address: Address,
        block_number: LenientBlockNumberOrTag,
        page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts>;

    /// Gets paginated inbound/outbound transaction calls for a certain address.
    ///
    /// Unimplemented: returns JSON-RPC error -32603 with message "unimplemented".
    /// See <https://github.com/paradigmxyz/reth/issues/13499>.
    #[method(name = "searchTransactionsAfter")]
    async fn search_transactions_after(
        &self,
        address: Address,
        block_number: LenientBlockNumberOrTag,
        page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts>;

    /// Gets the transaction hash for a certain sender address, given its nonce.
    #[method(name = "getTransactionBySenderAndNonce")]
    async fn get_transaction_by_sender_and_nonce(
        &self,
        sender: Address,
        nonce: u64,
    ) -> RpcResult<Option<TxHash>>;

    /// Gets the transaction hash and the address who created a contract.
    /// Requires historical state. Code-presence binary search is unreliable for destroyed and
    /// redeployed contracts, and cannot identify genesis allocations or EIP-7702 delegations.
    #[method(name = "getContractCreator")]
    async fn get_contract_creator(&self, address: Address) -> RpcResult<Option<ContractCreator>>;
}
