use alloy_json_rpc::RpcObject;
use alloy_primitives::{Address, Bytes, TxHash, B256};
use alloy_rpc_types::Header;
use alloy_rpc_types_trace::otterscan::{
    BlockDetails, ContractCreator, InternalOperation, OtsBlockTransactions, TraceEntry,
    TransactionsWithReceipts,
};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::BlockId;

/// Otterscan rpc interface.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "ots"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "ots"))]
pub trait Otterscan<T: RpcObject> {
    /// Get the block header by block number, required by otterscan.
    /// Otterscan currently requires this endpoint, used as:
    ///
    /// 1. check if the node is Erigon or not
    /// 2. get block header instead of the full block
    ///
    /// Ref: <https://github.com/otterscan/otterscan/blob/071d8c55202badf01804f6f8d53ef9311d4a9e47/src/useProvider.ts#L71>
    #[method(name = "getHeaderByNumber", aliases = ["erigon_getHeaderByNumber"])]
    async fn get_header_by_number(&self, block_number: u64) -> RpcResult<Option<Header>>;

    /// Check if a certain address contains a deployed code.
    #[method(name = "hasCode")]
    async fn has_code(&self, address: Address, block_id: Option<BlockId>) -> RpcResult<bool>;

    /// Very simple API versioning scheme. Every time we add a new capability, the number is
    /// incremented. This allows for Otterscan to check if the node contains all API it
    /// needs.
    #[method(name = "getApiLevel")]
    async fn get_api_level(&self) -> RpcResult<u64>;

    /// Return the internal ETH transfers inside a transaction.
    #[method(name = "getInternalOperations")]
    async fn get_internal_operations(&self, tx_hash: TxHash) -> RpcResult<Vec<InternalOperation>>;

    /// Given a transaction hash, returns its raw revert reason.
    #[method(name = "getTransactionError")]
    async fn get_transaction_error(&self, tx_hash: TxHash) -> RpcResult<Option<Bytes>>;

    /// Extract all variations of calls, contract creation and self-destructs and returns a call
    /// tree.
    #[method(name = "traceTransaction")]
    async fn trace_transaction(&self, tx_hash: TxHash) -> RpcResult<Option<Vec<TraceEntry>>>;

    /// Tailor-made and expanded version of eth_getBlockByNumber for block details page in
    /// Otterscan.
    #[method(name = "getBlockDetails")]
    async fn get_block_details(&self, block_number: u64) -> RpcResult<BlockDetails>;

    /// Tailor-made and expanded version of eth_getBlockByHash for block details page in Otterscan.
    #[method(name = "getBlockDetailsByHash")]
    async fn get_block_details_by_hash(&self, block_hash: B256) -> RpcResult<BlockDetails>;

    /// Get paginated transactions for a certain block. Also remove some verbose fields like logs.
    #[method(name = "getBlockTransactions")]
    async fn get_block_transactions(
        &self,
        block_number: u64,
        page_number: usize,
        page_size: usize,
    ) -> RpcResult<OtsBlockTransactions<T>>;

    /// Gets paginated inbound/outbound transaction calls for a certain address.
    #[method(name = "searchTransactionsBefore")]
    async fn search_transactions_before(
        &self,
        address: Address,
        block_number: u64,
        page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts>;

    /// Gets paginated inbound/outbound transaction calls for a certain address.
    #[method(name = "searchTransactionsAfter")]
    async fn search_transactions_after(
        &self,
        address: Address,
        block_number: u64,
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
    #[method(name = "getContractCreator")]
    async fn get_contract_creator(&self, address: Address) -> RpcResult<Option<ContractCreator>>;
}
