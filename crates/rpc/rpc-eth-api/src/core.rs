//! Implementation of the [`jsonrpsee`] generated [`EthApiServer`] trait. Handles RPC requests for
//! the `eth_` namespace.
use alloy_dyn_abi::TypedData;
use alloy_eips::eip2930::AccessListResult;
use alloy_json_rpc::RpcObject;
use alloy_primitives::{Address, Bytes, B256, B64, U256, U64};
use alloy_rpc_types::{
    serde_helpers::JsonStorageKey,
    simulate::{SimulatePayload, SimulatedBlock},
    state::{EvmOverrides, StateOverride},
    BlockOverrides, Bundle, EIP1186AccountProofResponse, EthCallResponse, FeeHistory, Header,
    Index, StateContext, SyncStatus, Work,
};
use alloy_rpc_types_eth::transaction::TransactionRequest;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::{BlockId, BlockNumberOrTag};
use reth_rpc_server_types::{result::internal_rpc_err, ToRpcResult};
use tracing::trace;

use crate::{
    helpers::{EthApiSpec, EthBlocks, EthCall, EthFees, EthState, EthTransactions, FullEthApi},
    RpcBlock, RpcReceipt, RpcTransaction,
};

/// Helper trait, unifies functionality that must be supported to implement all RPC methods for
/// server.
pub trait FullEthApiServer:
    EthApiServer<
        RpcTransaction<Self::NetworkTypes>,
        RpcBlock<Self::NetworkTypes>,
        RpcReceipt<Self::NetworkTypes>,
    > + FullEthApi
    + Clone
{
}

impl<T> FullEthApiServer for T where
    T: EthApiServer<
            RpcTransaction<T::NetworkTypes>,
            RpcBlock<T::NetworkTypes>,
            RpcReceipt<T::NetworkTypes>,
        > + FullEthApi
        + Clone
{
}

/// Eth rpc interface: <https://ethereum.github.io/execution-apis/api-documentation/>
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait EthApi<T: RpcObject, B: RpcObject, R: RpcObject> {
    /// Returns the protocol version encoded as a string.
    #[method(name = "protocolVersion")]
    async fn protocol_version(&self) -> RpcResult<U64>;

    /// Returns an object with data about the sync status or false.
    #[method(name = "syncing")]
    fn syncing(&self) -> RpcResult<SyncStatus>;

    /// Returns the client coinbase address.
    #[method(name = "coinbase")]
    async fn author(&self) -> RpcResult<Address>;

    /// Returns a list of addresses owned by client.
    #[method(name = "accounts")]
    fn accounts(&self) -> RpcResult<Vec<Address>>;

    /// Returns the number of most recent block.
    #[method(name = "blockNumber")]
    fn block_number(&self) -> RpcResult<U256>;

    /// Returns the chain ID of the current network.
    #[method(name = "chainId")]
    async fn chain_id(&self) -> RpcResult<Option<U64>>;

    /// Returns information about a block by hash.
    #[method(name = "getBlockByHash")]
    async fn block_by_hash(&self, hash: B256, full: bool) -> RpcResult<Option<B>>;

    /// Returns information about a block by number.
    #[method(name = "getBlockByNumber")]
    async fn block_by_number(&self, number: BlockNumberOrTag, full: bool) -> RpcResult<Option<B>>;

    /// Returns the number of transactions in a block from a block matching the given block hash.
    #[method(name = "getBlockTransactionCountByHash")]
    async fn block_transaction_count_by_hash(&self, hash: B256) -> RpcResult<Option<U256>>;

    /// Returns the number of transactions in a block matching the given block number.
    #[method(name = "getBlockTransactionCountByNumber")]
    async fn block_transaction_count_by_number(
        &self,
        number: BlockNumberOrTag,
    ) -> RpcResult<Option<U256>>;

    /// Returns the number of uncles in a block from a block matching the given block hash.
    #[method(name = "getUncleCountByBlockHash")]
    async fn block_uncles_count_by_hash(&self, hash: B256) -> RpcResult<Option<U256>>;

    /// Returns the number of uncles in a block with given block number.
    #[method(name = "getUncleCountByBlockNumber")]
    async fn block_uncles_count_by_number(
        &self,
        number: BlockNumberOrTag,
    ) -> RpcResult<Option<U256>>;

    /// Returns all transaction receipts for a given block.
    #[method(name = "getBlockReceipts")]
    async fn block_receipts(&self, block_id: BlockId) -> RpcResult<Option<Vec<R>>>;

    /// Returns an uncle block of the given block and index.
    #[method(name = "getUncleByBlockHashAndIndex")]
    async fn uncle_by_block_hash_and_index(&self, hash: B256, index: Index)
        -> RpcResult<Option<B>>;

    /// Returns an uncle block of the given block and index.
    #[method(name = "getUncleByBlockNumberAndIndex")]
    async fn uncle_by_block_number_and_index(
        &self,
        number: BlockNumberOrTag,
        index: Index,
    ) -> RpcResult<Option<B>>;

    /// Returns the EIP-2718 encoded transaction if it exists.
    ///
    /// If this is a EIP-4844 transaction that is in the pool it will include the sidecar.
    #[method(name = "getRawTransactionByHash")]
    async fn raw_transaction_by_hash(&self, hash: B256) -> RpcResult<Option<Bytes>>;

    /// Returns the information about a transaction requested by transaction hash.
    #[method(name = "getTransactionByHash")]
    async fn transaction_by_hash(&self, hash: B256) -> RpcResult<Option<T>>;

    /// Returns information about a raw transaction by block hash and transaction index position.
    #[method(name = "getRawTransactionByBlockHashAndIndex")]
    async fn raw_transaction_by_block_hash_and_index(
        &self,
        hash: B256,
        index: Index,
    ) -> RpcResult<Option<Bytes>>;

    /// Returns information about a transaction by block hash and transaction index position.
    #[method(name = "getTransactionByBlockHashAndIndex")]
    async fn transaction_by_block_hash_and_index(
        &self,
        hash: B256,
        index: Index,
    ) -> RpcResult<Option<T>>;

    /// Returns information about a raw transaction by block number and transaction index
    /// position.
    #[method(name = "getRawTransactionByBlockNumberAndIndex")]
    async fn raw_transaction_by_block_number_and_index(
        &self,
        number: BlockNumberOrTag,
        index: Index,
    ) -> RpcResult<Option<Bytes>>;

    /// Returns information about a transaction by block number and transaction index position.
    #[method(name = "getTransactionByBlockNumberAndIndex")]
    async fn transaction_by_block_number_and_index(
        &self,
        number: BlockNumberOrTag,
        index: Index,
    ) -> RpcResult<Option<T>>;

    /// Returns information about a transaction by sender and nonce.
    #[method(name = "getTransactionBySenderAndNonce")]
    async fn transaction_by_sender_and_nonce(
        &self,
        address: Address,
        nonce: U64,
    ) -> RpcResult<Option<T>>;

    /// Returns the receipt of a transaction by transaction hash.
    #[method(name = "getTransactionReceipt")]
    async fn transaction_receipt(&self, hash: B256) -> RpcResult<Option<R>>;

    /// Returns the balance of the account of given address.
    #[method(name = "getBalance")]
    async fn balance(&self, address: Address, block_number: Option<BlockId>) -> RpcResult<U256>;

    /// Returns the value from a storage position at a given address
    #[method(name = "getStorageAt")]
    async fn storage_at(
        &self,
        address: Address,
        index: JsonStorageKey,
        block_number: Option<BlockId>,
    ) -> RpcResult<B256>;

    /// Returns the number of transactions sent from an address at given block number.
    #[method(name = "getTransactionCount")]
    async fn transaction_count(
        &self,
        address: Address,
        block_number: Option<BlockId>,
    ) -> RpcResult<U256>;

    /// Returns code at a given address at given block number.
    #[method(name = "getCode")]
    async fn get_code(&self, address: Address, block_number: Option<BlockId>) -> RpcResult<Bytes>;

    /// Returns the block's header at given number.
    #[method(name = "getHeaderByNumber")]
    async fn header_by_number(&self, hash: BlockNumberOrTag) -> RpcResult<Option<Header>>;

    /// Returns the block's header at given hash.
    #[method(name = "getHeaderByHash")]
    async fn header_by_hash(&self, hash: B256) -> RpcResult<Option<Header>>;

    /// `eth_simulateV1` executes an arbitrary number of transactions on top of the requested state.
    /// The transactions are packed into individual blocks. Overrides can be provided.
    #[method(name = "simulateV1")]
    async fn simulate_v1(
        &self,
        opts: SimulatePayload,
        block_number: Option<BlockId>,
    ) -> RpcResult<Vec<SimulatedBlock<B>>>;

    /// Executes a new message call immediately without creating a transaction on the block chain.
    #[method(name = "call")]
    async fn call(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> RpcResult<Bytes>;

    /// Simulate arbitrary number of transactions at an arbitrary blockchain index, with the
    /// optionality of state overrides
    #[method(name = "callMany")]
    async fn call_many(
        &self,
        bundle: Bundle,
        state_context: Option<StateContext>,
        state_override: Option<StateOverride>,
    ) -> RpcResult<Vec<EthCallResponse>>;

    /// Generates an access list for a transaction.
    ///
    /// This method creates an [EIP2930](https://eips.ethereum.org/EIPS/eip-2930) type accessList based on a given Transaction.
    ///
    /// An access list contains all storage slots and addresses touched by the transaction, except
    /// for the sender account and the chain's precompiles.
    ///
    /// It returns list of addresses and storage keys used by the transaction, plus the gas
    /// consumed when the access list is added. That is, it gives you the list of addresses and
    /// storage keys that will be used by that transaction, plus the gas consumed if the access
    /// list is included. Like eth_estimateGas, this is an estimation; the list could change
    /// when the transaction is actually mined. Adding an accessList to your transaction does
    /// not necessary result in lower gas usage compared to a transaction without an access
    /// list.
    #[method(name = "createAccessList")]
    async fn create_access_list(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
    ) -> RpcResult<AccessListResult>;

    /// Generates and returns an estimate of how much gas is necessary to allow the transaction to
    /// complete.
    #[method(name = "estimateGas")]
    async fn estimate_gas(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_override: Option<StateOverride>,
    ) -> RpcResult<U256>;

    /// Returns the current price per gas in wei.
    #[method(name = "gasPrice")]
    async fn gas_price(&self) -> RpcResult<U256>;

    /// Returns the account details by specifying an address and a block number/tag
    #[method(name = "getAccount")]
    async fn get_account(
        &self,
        address: Address,
        block: BlockId,
    ) -> RpcResult<Option<alloy_rpc_types::Account>>;

    /// Introduced in EIP-1559, returns suggestion for the priority for dynamic fee transactions.
    #[method(name = "maxPriorityFeePerGas")]
    async fn max_priority_fee_per_gas(&self) -> RpcResult<U256>;

    /// Introduced in EIP-4844, returns the current blob base fee in wei.
    #[method(name = "blobBaseFee")]
    async fn blob_base_fee(&self) -> RpcResult<U256>;

    /// Returns the Transaction fee history
    ///
    /// Introduced in EIP-1559 for getting information on the appropriate priority fee to use.
    ///
    /// Returns transaction base fee per gas and effective priority fee per gas for the
    /// requested/supported block range. The returned Fee history for the returned block range
    /// can be a subsection of the requested range if not all blocks are available.
    #[method(name = "feeHistory")]
    async fn fee_history(
        &self,
        block_count: U64,
        newest_block: BlockNumberOrTag,
        reward_percentiles: Option<Vec<f64>>,
    ) -> RpcResult<FeeHistory>;

    /// Returns whether the client is actively mining new blocks.
    #[method(name = "mining")]
    async fn is_mining(&self) -> RpcResult<bool>;

    /// Returns the number of hashes per second that the node is mining with.
    #[method(name = "hashrate")]
    async fn hashrate(&self) -> RpcResult<U256>;

    /// Returns the hash of the current block, the seedHash, and the boundary condition to be met
    /// (“target”)
    #[method(name = "getWork")]
    async fn get_work(&self) -> RpcResult<Work>;

    /// Used for submitting mining hashrate.
    ///
    /// Can be used for remote miners to submit their hash rate.
    /// It accepts the miner hash rate and an identifier which must be unique between nodes.
    /// Returns `true` if the block was successfully submitted, `false` otherwise.
    #[method(name = "submitHashrate")]
    async fn submit_hashrate(&self, hashrate: U256, id: B256) -> RpcResult<bool>;

    /// Used for submitting a proof-of-work solution.
    #[method(name = "submitWork")]
    async fn submit_work(&self, nonce: B64, pow_hash: B256, mix_digest: B256) -> RpcResult<bool>;

    /// Sends transaction; will block waiting for signer to return the
    /// transaction hash.
    #[method(name = "sendTransaction")]
    async fn send_transaction(&self, request: TransactionRequest) -> RpcResult<B256>;

    /// Sends signed transaction, returning its hash.
    #[method(name = "sendRawTransaction")]
    async fn send_raw_transaction(&self, bytes: Bytes) -> RpcResult<B256>;

    /// Returns an Ethereum specific signature with: sign(keccak256("\x19Ethereum Signed Message:\n"
    /// + len(message) + message))).
    #[method(name = "sign")]
    async fn sign(&self, address: Address, message: Bytes) -> RpcResult<Bytes>;

    /// Signs a transaction that can be submitted to the network at a later time using with
    /// `sendRawTransaction.`
    #[method(name = "signTransaction")]
    async fn sign_transaction(&self, transaction: TransactionRequest) -> RpcResult<Bytes>;

    /// Signs data via [EIP-712](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-712.md).
    #[method(name = "signTypedData")]
    async fn sign_typed_data(&self, address: Address, data: TypedData) -> RpcResult<Bytes>;

    /// Returns the account and storage values of the specified account including the Merkle-proof.
    /// This call can be used to verify that the data you are pulling from is not tampered with.
    #[method(name = "getProof")]
    async fn get_proof(
        &self,
        address: Address,
        keys: Vec<JsonStorageKey>,
        block_number: Option<BlockId>,
    ) -> RpcResult<EIP1186AccountProofResponse>;
}

#[async_trait::async_trait]
impl<T>
    EthApiServer<
        RpcTransaction<T::NetworkTypes>,
        RpcBlock<T::NetworkTypes>,
        RpcReceipt<T::NetworkTypes>,
    > for T
where
    T: FullEthApi,
    jsonrpsee_types::error::ErrorObject<'static>: From<T::Error>,
{
    /// Handler for: `eth_protocolVersion`
    async fn protocol_version(&self) -> RpcResult<U64> {
        trace!(target: "rpc::eth", "Serving eth_protocolVersion");
        EthApiSpec::protocol_version(self).await.to_rpc_result()
    }

    /// Handler for: `eth_syncing`
    fn syncing(&self) -> RpcResult<SyncStatus> {
        trace!(target: "rpc::eth", "Serving eth_syncing");
        EthApiSpec::sync_status(self).to_rpc_result()
    }

    /// Handler for: `eth_coinbase`
    async fn author(&self) -> RpcResult<Address> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for: `eth_accounts`
    fn accounts(&self) -> RpcResult<Vec<Address>> {
        trace!(target: "rpc::eth", "Serving eth_accounts");
        Ok(EthApiSpec::accounts(self))
    }

    /// Handler for: `eth_blockNumber`
    fn block_number(&self) -> RpcResult<U256> {
        trace!(target: "rpc::eth", "Serving eth_blockNumber");
        Ok(U256::from(
            EthApiSpec::chain_info(self).with_message("failed to read chain info")?.best_number,
        ))
    }

    /// Handler for: `eth_chainId`
    async fn chain_id(&self) -> RpcResult<Option<U64>> {
        trace!(target: "rpc::eth", "Serving eth_chainId");
        Ok(Some(EthApiSpec::chain_id(self)))
    }

    /// Handler for: `eth_getBlockByHash`
    async fn block_by_hash(
        &self,
        hash: B256,
        full: bool,
    ) -> RpcResult<Option<RpcBlock<T::NetworkTypes>>> {
        trace!(target: "rpc::eth", ?hash, ?full, "Serving eth_getBlockByHash");
        Ok(EthBlocks::rpc_block(self, hash.into(), full).await?)
    }

    /// Handler for: `eth_getBlockByNumber`
    async fn block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> RpcResult<Option<RpcBlock<T::NetworkTypes>>> {
        trace!(target: "rpc::eth", ?number, ?full, "Serving eth_getBlockByNumber");
        Ok(EthBlocks::rpc_block(self, number.into(), full).await?)
    }

    /// Handler for: `eth_getBlockTransactionCountByHash`
    async fn block_transaction_count_by_hash(&self, hash: B256) -> RpcResult<Option<U256>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getBlockTransactionCountByHash");
        Ok(EthBlocks::block_transaction_count(self, hash.into()).await?.map(U256::from))
    }

    /// Handler for: `eth_getBlockTransactionCountByNumber`
    async fn block_transaction_count_by_number(
        &self,
        number: BlockNumberOrTag,
    ) -> RpcResult<Option<U256>> {
        trace!(target: "rpc::eth", ?number, "Serving eth_getBlockTransactionCountByNumber");
        Ok(EthBlocks::block_transaction_count(self, number.into()).await?.map(U256::from))
    }

    /// Handler for: `eth_getUncleCountByBlockHash`
    async fn block_uncles_count_by_hash(&self, hash: B256) -> RpcResult<Option<U256>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getUncleCountByBlockHash");
        Ok(EthBlocks::ommers(self, hash.into())?.map(|ommers| U256::from(ommers.len())))
    }

    /// Handler for: `eth_getUncleCountByBlockNumber`
    async fn block_uncles_count_by_number(
        &self,
        number: BlockNumberOrTag,
    ) -> RpcResult<Option<U256>> {
        trace!(target: "rpc::eth", ?number, "Serving eth_getUncleCountByBlockNumber");
        Ok(EthBlocks::ommers(self, number.into())?.map(|ommers| U256::from(ommers.len())))
    }

    /// Handler for: `eth_getBlockReceipts`
    async fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> RpcResult<Option<Vec<RpcReceipt<T::NetworkTypes>>>> {
        trace!(target: "rpc::eth", ?block_id, "Serving eth_getBlockReceipts");
        Ok(EthBlocks::block_receipts(self, block_id).await?)
    }

    /// Handler for: `eth_getUncleByBlockHashAndIndex`
    async fn uncle_by_block_hash_and_index(
        &self,
        hash: B256,
        index: Index,
    ) -> RpcResult<Option<RpcBlock<T::NetworkTypes>>> {
        trace!(target: "rpc::eth", ?hash, ?index, "Serving eth_getUncleByBlockHashAndIndex");
        Ok(EthBlocks::ommer_by_block_and_index(self, hash.into(), index).await?)
    }

    /// Handler for: `eth_getUncleByBlockNumberAndIndex`
    async fn uncle_by_block_number_and_index(
        &self,
        number: BlockNumberOrTag,
        index: Index,
    ) -> RpcResult<Option<RpcBlock<T::NetworkTypes>>> {
        trace!(target: "rpc::eth", ?number, ?index, "Serving eth_getUncleByBlockNumberAndIndex");
        Ok(EthBlocks::ommer_by_block_and_index(self, number.into(), index).await?)
    }

    /// Handler for: `eth_getRawTransactionByHash`
    async fn raw_transaction_by_hash(&self, hash: B256) -> RpcResult<Option<Bytes>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getRawTransactionByHash");
        Ok(EthTransactions::raw_transaction_by_hash(self, hash).await?)
    }

    /// Handler for: `eth_getTransactionByHash`
    async fn transaction_by_hash(
        &self,
        hash: B256,
    ) -> RpcResult<Option<RpcTransaction<T::NetworkTypes>>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getTransactionByHash");
        Ok(EthTransactions::transaction_by_hash(self, hash)
            .await?
            .map(|tx| tx.into_transaction::<T::TransactionCompat>()))
    }

    /// Handler for: `eth_getRawTransactionByBlockHashAndIndex`
    async fn raw_transaction_by_block_hash_and_index(
        &self,
        hash: B256,
        index: Index,
    ) -> RpcResult<Option<Bytes>> {
        trace!(target: "rpc::eth", ?hash, ?index, "Serving eth_getRawTransactionByBlockHashAndIndex");
        Ok(EthTransactions::raw_transaction_by_block_and_tx_index(self, hash.into(), index.into())
            .await?)
    }

    /// Handler for: `eth_getTransactionByBlockHashAndIndex`
    async fn transaction_by_block_hash_and_index(
        &self,
        hash: B256,
        index: Index,
    ) -> RpcResult<Option<RpcTransaction<T::NetworkTypes>>> {
        trace!(target: "rpc::eth", ?hash, ?index, "Serving eth_getTransactionByBlockHashAndIndex");
        Ok(EthTransactions::transaction_by_block_and_tx_index(self, hash.into(), index.into())
            .await?)
    }

    /// Handler for: `eth_getRawTransactionByBlockNumberAndIndex`
    async fn raw_transaction_by_block_number_and_index(
        &self,
        number: BlockNumberOrTag,
        index: Index,
    ) -> RpcResult<Option<Bytes>> {
        trace!(target: "rpc::eth", ?number, ?index, "Serving eth_getRawTransactionByBlockNumberAndIndex");
        Ok(EthTransactions::raw_transaction_by_block_and_tx_index(
            self,
            number.into(),
            index.into(),
        )
        .await?)
    }

    /// Handler for: `eth_getTransactionByBlockNumberAndIndex`
    async fn transaction_by_block_number_and_index(
        &self,
        number: BlockNumberOrTag,
        index: Index,
    ) -> RpcResult<Option<RpcTransaction<T::NetworkTypes>>> {
        trace!(target: "rpc::eth", ?number, ?index, "Serving eth_getTransactionByBlockNumberAndIndex");
        Ok(EthTransactions::transaction_by_block_and_tx_index(self, number.into(), index.into())
            .await?)
    }

    /// Handler for: `eth_getTransactionBySenderAndNonce`
    async fn transaction_by_sender_and_nonce(
        &self,
        sender: Address,
        nonce: U64,
    ) -> RpcResult<Option<RpcTransaction<T::NetworkTypes>>> {
        trace!(target: "rpc::eth", ?sender, ?nonce, "Serving eth_getTransactionBySenderAndNonce");
        Ok(EthTransactions::get_transaction_by_sender_and_nonce(self, sender, nonce.to(), true)
            .await?)
    }

    /// Handler for: `eth_getTransactionReceipt`
    async fn transaction_receipt(
        &self,
        hash: B256,
    ) -> RpcResult<Option<RpcReceipt<T::NetworkTypes>>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getTransactionReceipt");
        Ok(EthTransactions::transaction_receipt(self, hash).await?)
    }

    /// Handler for: `eth_getBalance`
    async fn balance(&self, address: Address, block_number: Option<BlockId>) -> RpcResult<U256> {
        trace!(target: "rpc::eth", ?address, ?block_number, "Serving eth_getBalance");
        Ok(EthState::balance(self, address, block_number).await?)
    }

    /// Handler for: `eth_getStorageAt`
    async fn storage_at(
        &self,
        address: Address,
        index: JsonStorageKey,
        block_number: Option<BlockId>,
    ) -> RpcResult<B256> {
        trace!(target: "rpc::eth", ?address, ?block_number, "Serving eth_getStorageAt");
        Ok(EthState::storage_at(self, address, index, block_number).await?)
    }

    /// Handler for: `eth_getTransactionCount`
    async fn transaction_count(
        &self,
        address: Address,
        block_number: Option<BlockId>,
    ) -> RpcResult<U256> {
        trace!(target: "rpc::eth", ?address, ?block_number, "Serving eth_getTransactionCount");
        Ok(EthState::transaction_count(self, address, block_number).await?)
    }

    /// Handler for: `eth_getCode`
    async fn get_code(&self, address: Address, block_number: Option<BlockId>) -> RpcResult<Bytes> {
        trace!(target: "rpc::eth", ?address, ?block_number, "Serving eth_getCode");
        Ok(EthState::get_code(self, address, block_number).await?)
    }

    /// Handler for: `eth_getHeaderByNumber`
    async fn header_by_number(&self, block_number: BlockNumberOrTag) -> RpcResult<Option<Header>> {
        trace!(target: "rpc::eth", ?block_number, "Serving eth_getHeaderByNumber");
        Ok(EthBlocks::rpc_block_header(self, block_number.into()).await?)
    }

    /// Handler for: `eth_getHeaderByHash`
    async fn header_by_hash(&self, hash: B256) -> RpcResult<Option<Header>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getHeaderByHash");
        Ok(EthBlocks::rpc_block_header(self, hash.into()).await?)
    }

    /// Handler for: `eth_simulateV1`
    async fn simulate_v1(
        &self,
        payload: SimulatePayload,
        block_number: Option<BlockId>,
    ) -> RpcResult<Vec<SimulatedBlock<RpcBlock<T::NetworkTypes>>>> {
        trace!(target: "rpc::eth", ?block_number, "Serving eth_simulateV1");
        Ok(EthCall::simulate_v1(self, payload, block_number).await?)
    }

    /// Handler for: `eth_call`
    async fn call(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> RpcResult<Bytes> {
        trace!(target: "rpc::eth", ?request, ?block_number, ?state_overrides, ?block_overrides, "Serving eth_call");
        Ok(EthCall::call(
            self,
            request,
            block_number,
            EvmOverrides::new(state_overrides, block_overrides),
        )
        .await?)
    }

    /// Handler for: `eth_callMany`
    async fn call_many(
        &self,
        bundle: Bundle,
        state_context: Option<StateContext>,
        state_override: Option<StateOverride>,
    ) -> RpcResult<Vec<EthCallResponse>> {
        trace!(target: "rpc::eth", ?bundle, ?state_context, ?state_override, "Serving eth_callMany");
        Ok(EthCall::call_many(self, bundle, state_context, state_override).await?)
    }

    /// Handler for: `eth_createAccessList`
    async fn create_access_list(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
    ) -> RpcResult<AccessListResult> {
        trace!(target: "rpc::eth", ?request, ?block_number, "Serving eth_createAccessList");
        Ok(EthCall::create_access_list_at(self, request, block_number).await?)
    }

    /// Handler for: `eth_estimateGas`
    async fn estimate_gas(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_override: Option<StateOverride>,
    ) -> RpcResult<U256> {
        trace!(target: "rpc::eth", ?request, ?block_number, "Serving eth_estimateGas");
        Ok(EthCall::estimate_gas_at(
            self,
            request,
            block_number.unwrap_or_default(),
            state_override,
        )
        .await?)
    }

    /// Handler for: `eth_gasPrice`
    async fn gas_price(&self) -> RpcResult<U256> {
        trace!(target: "rpc::eth", "Serving eth_gasPrice");
        Ok(EthFees::gas_price(self).await?)
    }

    /// Handler for: `eth_getAccount`
    async fn get_account(
        &self,
        address: Address,
        block: BlockId,
    ) -> RpcResult<Option<alloy_rpc_types::Account>> {
        trace!(target: "rpc::eth", "Serving eth_getAccount");
        Ok(EthState::get_account(self, address, block).await?)
    }

    /// Handler for: `eth_maxPriorityFeePerGas`
    async fn max_priority_fee_per_gas(&self) -> RpcResult<U256> {
        trace!(target: "rpc::eth", "Serving eth_maxPriorityFeePerGas");
        Ok(EthFees::suggested_priority_fee(self).await?)
    }

    /// Handler for: `eth_blobBaseFee`
    async fn blob_base_fee(&self) -> RpcResult<U256> {
        trace!(target: "rpc::eth", "Serving eth_blobBaseFee");
        Ok(EthFees::blob_base_fee(self).await?)
    }

    // FeeHistory is calculated based on lazy evaluation of fees for historical blocks, and further
    // caching of it in the LRU cache.
    // When new RPC call is executed, the cache gets locked, we check it for the historical fees
    // according to the requested block range, and fill any cache misses (in both RPC response
    // and cache itself) with the actual data queried from the database.
    // To minimize the number of database seeks required to query the missing data, we calculate the
    // first non-cached block number and last non-cached block number. After that, we query this
    // range of consecutive blocks from the database.
    /// Handler for: `eth_feeHistory`
    async fn fee_history(
        &self,
        block_count: U64,
        newest_block: BlockNumberOrTag,
        reward_percentiles: Option<Vec<f64>>,
    ) -> RpcResult<FeeHistory> {
        trace!(target: "rpc::eth", ?block_count, ?newest_block, ?reward_percentiles, "Serving eth_feeHistory");
        Ok(EthFees::fee_history(self, block_count.to(), newest_block, reward_percentiles).await?)
    }

    /// Handler for: `eth_mining`
    async fn is_mining(&self) -> RpcResult<bool> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for: `eth_hashrate`
    async fn hashrate(&self) -> RpcResult<U256> {
        Ok(U256::ZERO)
    }

    /// Handler for: `eth_getWork`
    async fn get_work(&self) -> RpcResult<Work> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for: `eth_submitHashrate`
    async fn submit_hashrate(&self, _hashrate: U256, _id: B256) -> RpcResult<bool> {
        Ok(false)
    }

    /// Handler for: `eth_submitWork`
    async fn submit_work(
        &self,
        _nonce: B64,
        _pow_hash: B256,
        _mix_digest: B256,
    ) -> RpcResult<bool> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for: `eth_sendTransaction`
    async fn send_transaction(&self, request: TransactionRequest) -> RpcResult<B256> {
        trace!(target: "rpc::eth", ?request, "Serving eth_sendTransaction");
        Ok(EthTransactions::send_transaction(self, request).await?)
    }

    /// Handler for: `eth_sendRawTransaction`
    async fn send_raw_transaction(&self, tx: Bytes) -> RpcResult<B256> {
        trace!(target: "rpc::eth", ?tx, "Serving eth_sendRawTransaction");
        Ok(EthTransactions::send_raw_transaction(self, tx).await?)
    }

    /// Handler for: `eth_sign`
    async fn sign(&self, address: Address, message: Bytes) -> RpcResult<Bytes> {
        trace!(target: "rpc::eth", ?address, ?message, "Serving eth_sign");
        Ok(EthTransactions::sign(self, address, message).await?)
    }

    /// Handler for: `eth_signTransaction`
    async fn sign_transaction(&self, _transaction: TransactionRequest) -> RpcResult<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for: `eth_signTypedData`
    async fn sign_typed_data(&self, address: Address, data: TypedData) -> RpcResult<Bytes> {
        trace!(target: "rpc::eth", ?address, ?data, "Serving eth_signTypedData");
        Ok(EthTransactions::sign_typed_data(self, &data, address)?)
    }

    /// Handler for: `eth_getProof`
    async fn get_proof(
        &self,
        address: Address,
        keys: Vec<JsonStorageKey>,
        block_number: Option<BlockId>,
    ) -> RpcResult<EIP1186AccountProofResponse> {
        trace!(target: "rpc::eth", ?address, ?keys, ?block_number, "Serving eth_getProof");
        Ok(EthState::get_proof(self, address, keys, block_number)?.await?)
    }
}
