use jsonrpsee::{core::RpcResult as Result, proc_macros::rpc};
use reth_primitives::{Address, BlockNumber, Bytes, H256, H64, U256, U64};
use reth_rpc_types::{
    CallRequest, FeeHistory, Index, RichBlock, SyncStatus, Transaction, TransactionReceipt,
    TransactionRequest, Work,
};

/// Eth rpc interface.
#[rpc(server)]
#[async_trait]
pub trait EthApi {
    /// Returns protocol version encoded as a string (quotes are necessary).
    #[method(name = "eth_protocolVersion")]
    fn protocol_version(&self) -> Result<u64>;

    /// Returns an object with data about the sync status or false. (wtf?)
    #[method(name = "eth_syncing")]
    fn syncing(&self) -> Result<SyncStatus>;

    /// Returns block author.
    #[method(name = "eth_coinbase")]
    fn author(&self) -> Result<Address>;

    /// Returns accounts list.
    #[method(name = "eth_accounts")]
    fn accounts(&self) -> Result<Vec<Address>>;

    /// Returns highest block number.
    #[method(name = "eth_blockNumber")]
    fn block_number(&self) -> Result<U256>;

    /// Returns the chain ID used for transaction signing at the
    /// current best block. None is returned if not
    /// available.
    #[method(name = "eth_chainId")]
    fn chain_id(&self) -> Result<Option<U64>>;

    /// Returns block with given hash.
    #[method(name = "eth_getBlockByHash")]
    async fn block_by_hash(&self, hash: H256, full: bool) -> Result<Option<RichBlock>>;

    /// Returns block with given number.
    #[method(name = "eth_getBlockByNumber")]
    async fn block_by_number(&self, number: BlockNumber, full: bool) -> Result<Option<RichBlock>>;

    /// Returns the number of transactions in a block with given hash.
    #[method(name = "eth_getBlockTransactionCountByHash")]
    fn block_transaction_count_by_hash(&self, hash: H256) -> Result<Option<U256>>;

    /// Returns the number of transactions in a block with given block number.
    #[method(name = "eth_getBlockTransactionCountByNumber")]
    fn block_transaction_count_by_number(&self, number: BlockNumber) -> Result<Option<U256>>;

    /// Returns the number of uncles in a block with given hash.
    #[method(name = "eth_getUncleCountByBlockHash")]
    fn block_uncles_count_by_hash(&self, hash: H256) -> Result<U256>;

    /// Returns the number of uncles in a block with given block number.
    #[method(name = "eth_getUncleCountByBlockNumber")]
    fn block_uncles_count_by_number(&self, number: BlockNumber) -> Result<U256>;

    /// Returns an uncles at given block and index.
    #[method(name = "eth_getUncleByBlockHashAndIndex")]
    fn uncle_by_block_hash_and_index(&self, hash: H256, index: Index) -> Result<Option<RichBlock>>;

    /// Returns an uncles at given block and index.
    #[method(name = "eth_getUncleByBlockNumberAndIndex")]
    fn uncle_by_block_number_and_index(
        &self,
        number: BlockNumber,
        index: Index,
    ) -> Result<Option<RichBlock>>;

    /// Get transaction by its hash.
    #[method(name = "eth_getTransactionByHash")]
    async fn transaction_by_hash(&self, hash: H256) -> Result<Option<Transaction>>;

    /// Returns transaction at given block hash and index.
    #[method(name = "eth_getTransactionByBlockHashAndIndex")]
    async fn transaction_by_block_hash_and_index(
        &self,
        hash: H256,
        index: Index,
    ) -> Result<Option<Transaction>>;

    /// Returns transaction by given block number and index.
    #[method(name = "eth_getTransactionByBlockNumberAndIndex")]
    async fn transaction_by_block_number_and_index(
        &self,
        number: BlockNumber,
        index: Index,
    ) -> Result<Option<Transaction>>;

    /// Returns transaction receipt by transaction hash.
    #[method(name = "eth_getTransactionReceipt")]
    async fn transaction_receipt(&self, hash: H256) -> Result<Option<TransactionReceipt>>;

    /// Returns balance of the given account.
    #[method(name = "eth_getBalance")]
    fn balance(&self, address: Address, number: Option<BlockNumber>) -> Result<U256>;

    /// Returns content of the storage at given address.
    #[method(name = "eth_getStorageAt")]
    fn storage_at(
        &self,
        address: Address,
        index: U256,
        number: Option<BlockNumber>,
    ) -> Result<H256>;

    /// Returns the number of transactions sent from given address at given time (block number).
    #[method(name = "eth_getTransactionCount")]
    fn transaction_count(&self, address: Address, number: Option<BlockNumber>) -> Result<U256>;

    /// Returns the code at given address at given time (block number).
    #[method(name = "eth_getCode")]
    fn code_at(&self, address: Address, number: Option<BlockNumber>) -> Result<Bytes>;

    /// Call contract, returning the output data.
    #[method(name = "eth_call")]
    fn call(&self, request: CallRequest, number: Option<BlockNumber>) -> Result<Bytes>;

    /// Estimate gas needed for execution of given contract.
    #[method(name = "eth_estimateGas")]
    async fn estimate_gas(&self, request: CallRequest, number: Option<BlockNumber>)
        -> Result<U256>;

    /// Returns current gas_price.
    #[method(name = "eth_gasPrice")]
    fn gas_price(&self) -> Result<U256>;

    /// Introduced in EIP-1159 for getting information on the appropriate priority fee to use.
    #[method(name = "eth_feeHistory")]
    fn fee_history(
        &self,
        block_count: U256,
        newest_block: BlockNumber,
        reward_percentiles: Option<Vec<f64>>,
    ) -> Result<FeeHistory>;

    /// Introduced in EIP-1159, a Geth-specific and simplified priority fee oracle.
    /// Leverages the already existing fee history cache.
    #[method(name = "eth_maxPriorityFeePerGas")]
    fn max_priority_fee_per_gas(&self) -> Result<U256>;

    /// Returns true if client is actively mining new blocks.
    #[method(name = "eth_mining")]
    fn is_mining(&self) -> Result<bool>;

    /// Returns the number of hashes per second that the node is mining with.
    #[method(name = "eth_hashrate")]
    fn hashrate(&self) -> Result<U256>;

    /// Returns the hash of the current block, the seedHash, and the boundary condition to be met.
    #[method(name = "eth_getWork")]
    fn work(&self) -> Result<Work>;

    /// Used for submitting mining hashrate.
    #[method(name = "eth_submitHashrate")]
    fn submit_hashrate(&self, hashrate: U256, id: H256) -> Result<bool>;

    /// Used for submitting a proof-of-work solution.
    #[method(name = "eth_submitWork")]
    fn submit_work(&self, nonce: H64, pow_hash: H256, mix_digest: H256) -> Result<bool>;

    /// Sends transaction; will block waiting for signer to return the
    /// transaction hash.
    #[method(name = "eth_sendTransaction")]
    async fn send_transaction(&self, request: TransactionRequest) -> Result<H256>;

    /// Sends signed transaction, returning its hash.
    #[method(name = "eth_sendRawTransaction")]
    async fn send_raw_transaction(&self, bytes: Bytes) -> Result<H256>;
}
