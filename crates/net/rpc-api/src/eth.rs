use jsonrpsee::{core::RpcResult as Result, proc_macros::rpc};
use reth_primitives::{
    rpc::{transaction::eip2930::AccessListWithGasUsed, BlockId},
    Address, BlockNumber, Bytes, H256, H64, U256, U64,
};
use reth_rpc_types::{
    CallRequest, EIP1186AccountProofResponse, FeeHistory, Index, RichBlock, SyncStatus,
    Transaction, TransactionReceipt, TransactionRequest, Work,
};

/// Eth rpc interface: <https://ethereum.github.io/execution-apis/api-documentation/>
#[cfg_attr(not(feature = "client"), rpc(server))]
#[cfg_attr(feature = "client", rpc(server, client))]
#[async_trait]
pub trait EthApi {
    /// Returns protocol version encoded as a string.
    #[method(name = "eth_protocolVersion")]
    fn protocol_version(&self) -> Result<U64>;

    /// Returns an object with data about the sync status or false.
    #[method(name = "eth_syncing")]
    fn syncing(&self) -> Result<SyncStatus>;

    /// Returns block author.
    #[method(name = "eth_coinbase")]
    async fn author(&self) -> Result<Address>;

    /// Returns accounts list.
    #[method(name = "eth_accounts")]
    async fn accounts(&self) -> Result<Vec<Address>>;

    /// Returns the best block number.
    #[method(name = "eth_blockNumber")]
    fn block_number(&self) -> Result<U256>;

    /// Returns the chain ID used for transaction signing at the
    /// current best block. None is returned if not
    /// available.
    #[method(name = "eth_chainId")]
    async fn chain_id(&self) -> Result<Option<U64>>;

    /// Returns block with given hash.
    #[method(name = "eth_getBlockByHash")]
    async fn block_by_hash(&self, hash: H256, full: bool) -> Result<Option<RichBlock>>;

    /// Returns block with given number.
    #[method(name = "eth_getBlockByNumber")]
    async fn block_by_number(&self, number: BlockNumber, full: bool) -> Result<Option<RichBlock>>;

    /// Returns the number of transactions in a block with given hash.
    #[method(name = "eth_getBlockTransactionCountByHash")]
    async fn block_transaction_count_by_hash(&self, hash: H256) -> Result<Option<U256>>;

    /// Returns the number of transactions in a block with given block number.
    #[method(name = "eth_getBlockTransactionCountByNumber")]
    async fn block_transaction_count_by_number(&self, number: BlockNumber) -> Result<Option<U256>>;

    /// Returns the number of uncles in a block with given hash.
    #[method(name = "eth_getUncleCountByBlockHash")]
    async fn block_uncles_count_by_hash(&self, hash: H256) -> Result<U256>;

    /// Returns the number of uncles in a block with given block number.
    #[method(name = "eth_getUncleCountByBlockNumber")]
    async fn block_uncles_count_by_number(&self, number: BlockNumber) -> Result<U256>;

    /// Returns an uncles at given block and index.
    #[method(name = "eth_getUncleByBlockHashAndIndex")]
    async fn uncle_by_block_hash_and_index(
        &self,
        hash: H256,
        index: Index,
    ) -> Result<Option<RichBlock>>;

    /// Returns an uncles at given block and index.
    #[method(name = "eth_getUncleByBlockNumberAndIndex")]
    async fn uncle_by_block_number_and_index(
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
    async fn balance(&self, address: Address, block_number: Option<BlockId>) -> Result<U256>;

    /// Returns content of the storage at given address.
    #[method(name = "eth_getStorageAt")]
    async fn storage_at(
        &self,
        address: Address,
        index: U256,
        block_number: Option<BlockId>,
    ) -> Result<H256>;

    /// Returns the number of transactions sent from given address at given time (block number).
    #[method(name = "eth_getTransactionCount")]
    async fn transaction_count(
        &self,
        address: Address,
        block_number: Option<BlockId>,
    ) -> Result<U256>;

    /// Returns the code at given address at given time (block number).
    #[method(name = "eth_getCode")]
    async fn code_at(&self, address: Address, block_number: Option<BlockId>) -> Result<Bytes>;

    /// Call contract, returning the output data.
    #[method(name = "eth_call")]
    async fn call(&self, request: CallRequest, block_number: Option<BlockId>) -> Result<Bytes>;

    /// This method creates an EIP2930 type accessList based on a given Transaction. The accessList
    /// contains all storage slots and addresses read and written by the transaction, except for the
    /// sender account and the precompiles.
    ///
    /// It returns list of addresses and storage keys used by the transaction, plus the gas
    /// consumed when the access list is added. That is, it gives you the list of addresses and
    /// storage keys that will be used by that transaction, plus the gas consumed if the access
    /// list is included. Like eth_estimateGas, this is an estimation; the list could change
    /// when the transaction is actually mined. Adding an accessList to your transaction does
    /// not necessary result in lower gas usage compared to a transaction without an access
    /// list.
    #[method(name = "eth_createAccessList")]
    async fn create_access_list(
        &self,
        request: CallRequest,
        block_number: Option<BlockId>,
    ) -> Result<AccessListWithGasUsed>;

    /// Estimate gas needed for execution of given contract.
    #[method(name = "eth_estimateGas")]
    async fn estimate_gas(
        &self,
        request: CallRequest,
        block_number: Option<BlockId>,
    ) -> Result<U256>;

    /// Returns current gas_price.
    #[method(name = "eth_gasPrice")]
    async fn gas_price(&self) -> Result<U256>;

    /// Introduced in EIP-1159 for getting information on the appropriate priority fee to use.
    #[method(name = "eth_feeHistory")]
    async fn fee_history(
        &self,
        block_count: U256,
        newest_block: BlockNumber,
        reward_percentiles: Option<Vec<f64>>,
    ) -> Result<FeeHistory>;

    /// Introduced in EIP-1159, a Geth-specific and simplified priority fee oracle.
    /// Leverages the already existing fee history cache.
    #[method(name = "eth_maxPriorityFeePerGas")]
    async fn max_priority_fee_per_gas(&self) -> Result<U256>;

    /// Returns true if client is actively mining new blocks.
    #[method(name = "eth_mining")]
    async fn is_mining(&self) -> Result<bool>;

    /// Returns the number of hashes per second that the node is mining with.
    #[method(name = "eth_hashrate")]
    async fn hashrate(&self) -> Result<U256>;

    /// Returns the hash of the current block, the seedHash, and the boundary condition to be met.
    #[method(name = "eth_getWork")]
    async fn work(&self) -> Result<Work>;

    /// Used for submitting mining hashrate.
    #[method(name = "eth_submitHashrate")]
    async fn submit_hashrate(&self, hashrate: U256, id: H256) -> Result<bool>;

    /// Used for submitting a proof-of-work solution.
    #[method(name = "eth_submitWork")]
    async fn submit_work(&self, nonce: H64, pow_hash: H256, mix_digest: H256) -> Result<bool>;

    /// Sends transaction; will block waiting for signer to return the
    /// transaction hash.
    #[method(name = "eth_sendTransaction")]
    async fn send_transaction(&self, request: TransactionRequest) -> Result<H256>;

    /// Sends signed transaction, returning its hash.
    #[method(name = "eth_sendRawTransaction")]
    async fn send_raw_transaction(&self, bytes: Bytes) -> Result<H256>;

    /// Returns an Ethereum specific signature with: sign(keccak256("\x19Ethereum Signed Message:\n"
    /// + len(message) + message))).
    #[method(name = "eth_sign")]
    async fn sign(&self, address: Address, message: Bytes) -> Result<Bytes>;

    /// Signs a transaction that can be submitted to the network at a later time using with
    /// `eth_sendRawTransaction.`
    #[method(name = "eth_signTransaction")]
    async fn sign_transaction(&self, transaction: CallRequest) -> Result<Bytes>;

    /// Signs data via [EIP-712](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-712.md).
    #[method(name = "eth_signTypedData")]
    async fn sign_typed_data(&self, address: Address, data: serde_json::Value) -> Result<Bytes>;

    /// Returns the account and storage values of the specified account including the Merkle-proof.
    /// This call can be used to verify that the data you are pulling from is not tampered with.
    #[method(name = "eth_getProof")]
    async fn get_proof(
        &self,
        address: Address,
        keys: Vec<H256>,
        block_number: Option<BlockId>,
    ) -> Result<EIP1186AccountProofResponse>;
}
