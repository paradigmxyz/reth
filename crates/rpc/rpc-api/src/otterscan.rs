use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::{Address, BlockId, TxHash, H256, U256};
use reth_rpc_types::{
    BlockDetails, ContractCreator, InternalOperation, TraceEntry, Transaction,
    TransactionsWithReceipts,
};

/// Otterscan rpc interface.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "ots"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "ots"))]
pub trait Otterscan {
    /// Check if a certain address contains a deployed code.
    #[method(name = "hasCode")]
    async fn has_code(&self, address: Address, block_number: Option<BlockId>) -> RpcResult<bool>;

    /// Very simple API versioning scheme. Every time we add a new capability, the number is
    /// incremented. This allows for Otterscan to check if the Erigon node contains all API it
    /// needs.
    #[method(name = "getApiLevel")]
    async fn get_api_level(&self) -> RpcResult<u64>;

    /// Return the internal ETH transfers inside a transaction.
    #[method(name = "getInternalOperations")]
    async fn get_internal_operations(&self, tx_hash: TxHash) -> RpcResult<Vec<InternalOperation>>;

    /// Given a transaction hash, returns its raw revert reason.
    #[method(name = "getTransactionError")]
    async fn get_transaction_error(&self, tx_hash: TxHash) -> RpcResult<String>;

    /// Extract all variations of calls, contract creation and self-destructs and returns a call
    /// tree.
    #[method(name = "traceTransaction")]
    async fn trace_transaction(&self, tx_hash: TxHash) -> RpcResult<TraceEntry>;

    /// Tailor-made and expanded version of eth_getBlockByNumber for block details page in
    /// Otterscan.
    #[method(name = "getBlockDetails")]
    async fn get_block_details(&self, block_number: U256) -> RpcResult<BlockDetails>;

    /// Tailor-made and expanded version of eth_getBlockByHash for block details page in Otterscan.
    #[method(name = "getBlockDetailsByHash")]
    async fn get_block_details_by_hash(&self, block_hash: H256) -> RpcResult<BlockDetails>;

    /// Get paginated transactions for a certain block. Also remove some verbose fields like logs.
    #[method(name = "getBlockTransactions")]
    async fn get_block_transactions(
        &self,
        block_number: U256,
        page_number: u8,
        page_size: u8,
    ) -> RpcResult<Vec<Transaction>>;

    //custom return
    /// Gets paginated inbound/outbound transaction calls for a certain address.
    #[method(name = "searchTransactionsBefore")]
    async fn search_transactions_before(
        &self,
        address: Address,
        block_number: U256,
        page_size: u8,
    ) -> RpcResult<TransactionsWithReceipts>;

    //custom return
    /// Gets paginated inbound/outbound transaction calls for a certain address.
    #[method(name = "searchTransactionsAfter")]
    async fn search_transactions_after(
        &self,
        address: Address,
        block_number: U256,
        page_size: u8,
    ) -> RpcResult<TransactionsWithReceipts>;

    /// Gets the transaction hash for a certain sender address, given its nonce.
    #[method(name = "getTransactionBySenderAndNonce")]
    async fn get_transaction_by_sender_and_nonce(
        &self,
        sender: Address,
        nonce: u64,
    ) -> RpcResult<Transaction>;

    //custom return
    /// Gets the transaction hash and the address who created a contract.
    #[method(name = "getContractCreator")]
    async fn get_contract_creator(&self, address: Address) -> RpcResult<ContractCreator>;
}
