//! Implementation of the [`jsonrpsee`] generated [`reth_rpc_api::EthApiServer`] trait
//! Handles RPC requests for the `eth_` namespace.

use super::EthApiSpec;
use crate::{
    eth::{
        api::{EthApi, EthTransactions},
        error::EthApiError,
        revm_utils::EvmOverrides,
    },
    result::{internal_rpc_err, ToRpcResult},
};
use jsonrpsee::core::RpcResult as Result;
use reth_network_api::NetworkInfo;
use reth_node_api::ConfigureEvmEnv;
use reth_primitives::{
    serde_helper::{num::U64HexOrNumber, JsonStorageKey},
    Address, BlockId, BlockNumberOrTag, Bytes, B256, B64, U256, U64,
};
use reth_provider::{
    BlockIdReader, BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider,
    HeaderProvider, StateProviderFactory,
};
use reth_rpc_api::EthApiServer;
use reth_rpc_types::{
    state::StateOverride, AccessListWithGasUsed, BlockOverrides, Bundle,
    EIP1186AccountProofResponse, EthCallResponse, FeeHistory, Header, Index, RichBlock,
    StateContext, SyncStatus, TransactionReceipt, TransactionRequest, Work,
};
use reth_transaction_pool::TransactionPool;
use serde_json::Value;
use tracing::trace;

#[async_trait::async_trait]
impl<Provider, Pool, Network, EvmConfig> EthApiServer for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: EthApiSpec + EthTransactions,
    Pool: TransactionPool + 'static,
    Provider: BlockReader
        + BlockIdReader
        + BlockReaderIdExt
        + ChainSpecProvider
        + HeaderProvider
        + StateProviderFactory
        + EvmEnvProvider
        + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
    EvmConfig: ConfigureEvmEnv + 'static,
{
    /// Handler for: `eth_protocolVersion`
    async fn protocol_version(&self) -> Result<U64> {
        trace!(target: "rpc::eth", "Serving eth_protocolVersion");
        EthApiSpec::protocol_version(self).await.to_rpc_result()
    }

    /// Handler for: `eth_syncing`
    fn syncing(&self) -> Result<SyncStatus> {
        trace!(target: "rpc::eth", "Serving eth_syncing");
        EthApiSpec::sync_status(self).to_rpc_result()
    }

    /// Handler for: `eth_coinbase`
    async fn author(&self) -> Result<Address> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for: `eth_accounts`
    fn accounts(&self) -> Result<Vec<Address>> {
        trace!(target: "rpc::eth", "Serving eth_accounts");
        Ok(EthApiSpec::accounts(self))
    }

    /// Handler for: `eth_blockNumber`
    fn block_number(&self) -> Result<U256> {
        trace!(target: "rpc::eth", "Serving eth_blockNumber");
        Ok(U256::from(
            EthApiSpec::chain_info(self).with_message("failed to read chain info")?.best_number,
        ))
    }

    /// Handler for: `eth_chainId`
    async fn chain_id(&self) -> Result<Option<U64>> {
        trace!(target: "rpc::eth", "Serving eth_chainId");
        Ok(Some(EthApiSpec::chain_id(self)))
    }

    /// Handler for: `eth_getBlockByHash`
    async fn block_by_hash(&self, hash: B256, full: bool) -> Result<Option<RichBlock>> {
        trace!(target: "rpc::eth", ?hash, ?full, "Serving eth_getBlockByHash");
        Ok(EthApi::rpc_block(self, hash, full).await?)
    }

    /// Handler for: `eth_getBlockByNumber`
    async fn block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> Result<Option<RichBlock>> {
        trace!(target: "rpc::eth", ?number, ?full, "Serving eth_getBlockByNumber");
        Ok(EthApi::rpc_block(self, number, full).await?)
    }

    /// Handler for: `eth_getBlockTransactionCountByHash`
    async fn block_transaction_count_by_hash(&self, hash: B256) -> Result<Option<U256>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getBlockTransactionCountByHash");
        Ok(EthApi::block_transaction_count(self, hash).await?.map(U256::from))
    }

    /// Handler for: `eth_getBlockTransactionCountByNumber`
    async fn block_transaction_count_by_number(
        &self,
        number: BlockNumberOrTag,
    ) -> Result<Option<U256>> {
        trace!(target: "rpc::eth", ?number, "Serving eth_getBlockTransactionCountByNumber");
        Ok(EthApi::block_transaction_count(self, number).await?.map(U256::from))
    }

    /// Handler for: `eth_getUncleCountByBlockHash`
    async fn block_uncles_count_by_hash(&self, hash: B256) -> Result<Option<U256>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getUncleCountByBlockHash");
        Ok(EthApi::ommers(self, hash)?.map(|ommers| U256::from(ommers.len())))
    }

    /// Handler for: `eth_getUncleCountByBlockNumber`
    async fn block_uncles_count_by_number(&self, number: BlockNumberOrTag) -> Result<Option<U256>> {
        trace!(target: "rpc::eth", ?number, "Serving eth_getUncleCountByBlockNumber");
        Ok(EthApi::ommers(self, number)?.map(|ommers| U256::from(ommers.len())))
    }

    /// Handler for: `eth_getBlockReceipts`
    async fn block_receipts(&self, block_id: BlockId) -> Result<Option<Vec<TransactionReceipt>>> {
        trace!(target: "rpc::eth", ?block_id, "Serving eth_getBlockReceipts");
        Ok(EthApi::block_receipts(self, block_id).await?)
    }

    /// Handler for: `eth_getUncleByBlockHashAndIndex`
    async fn uncle_by_block_hash_and_index(
        &self,
        hash: B256,
        index: Index,
    ) -> Result<Option<RichBlock>> {
        trace!(target: "rpc::eth", ?hash, ?index, "Serving eth_getUncleByBlockHashAndIndex");
        Ok(EthApi::ommer_by_block_and_index(self, hash, index).await?)
    }

    /// Handler for: `eth_getUncleByBlockNumberAndIndex`
    async fn uncle_by_block_number_and_index(
        &self,
        number: BlockNumberOrTag,
        index: Index,
    ) -> Result<Option<RichBlock>> {
        trace!(target: "rpc::eth", ?number, ?index, "Serving eth_getUncleByBlockNumberAndIndex");
        Ok(EthApi::ommer_by_block_and_index(self, number, index).await?)
    }

    /// Handler for: `eth_getRawTransactionByHash`
    async fn raw_transaction_by_hash(&self, hash: B256) -> Result<Option<Bytes>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getRawTransactionByHash");
        Ok(EthTransactions::raw_transaction_by_hash(self, hash).await?)
    }

    /// Handler for: `eth_getTransactionByHash`
    async fn transaction_by_hash(&self, hash: B256) -> Result<Option<reth_rpc_types::Transaction>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getTransactionByHash");
        Ok(EthTransactions::transaction_by_hash(self, hash).await?.map(Into::into))
    }

    /// Handler for: `eth_getRawTransactionByBlockHashAndIndex`
    async fn raw_transaction_by_block_hash_and_index(
        &self,
        hash: B256,
        index: Index,
    ) -> Result<Option<Bytes>> {
        trace!(target: "rpc::eth", ?hash, ?index, "Serving eth_getRawTransactionByBlockHashAndIndex");
        Ok(EthApi::raw_transaction_by_block_and_tx_index(self, hash, index).await?)
    }

    /// Handler for: `eth_getTransactionByBlockHashAndIndex`
    async fn transaction_by_block_hash_and_index(
        &self,
        hash: B256,
        index: Index,
    ) -> Result<Option<reth_rpc_types::Transaction>> {
        trace!(target: "rpc::eth", ?hash, ?index, "Serving eth_getTransactionByBlockHashAndIndex");
        Ok(EthApi::transaction_by_block_and_tx_index(self, hash, index).await?)
    }

    /// Handler for: `eth_getRawTransactionByBlockNumberAndIndex`
    async fn raw_transaction_by_block_number_and_index(
        &self,
        number: BlockNumberOrTag,
        index: Index,
    ) -> Result<Option<Bytes>> {
        trace!(target: "rpc::eth", ?number, ?index, "Serving eth_getRawTransactionByBlockNumberAndIndex");
        Ok(EthApi::raw_transaction_by_block_and_tx_index(self, number, index).await?)
    }

    /// Handler for: `eth_getTransactionByBlockNumberAndIndex`
    async fn transaction_by_block_number_and_index(
        &self,
        number: BlockNumberOrTag,
        index: Index,
    ) -> Result<Option<reth_rpc_types::Transaction>> {
        trace!(target: "rpc::eth", ?number, ?index, "Serving eth_getTransactionByBlockNumberAndIndex");
        Ok(EthApi::transaction_by_block_and_tx_index(self, number, index).await?)
    }

    /// Handler for: `eth_getTransactionReceipt`
    async fn transaction_receipt(&self, hash: B256) -> Result<Option<TransactionReceipt>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getTransactionReceipt");
        Ok(EthTransactions::transaction_receipt(self, hash).await?)
    }

    /// Handler for: `eth_getBalance`
    async fn balance(&self, address: Address, block_number: Option<BlockId>) -> Result<U256> {
        trace!(target: "rpc::eth", ?address, ?block_number, "Serving eth_getBalance");
        Ok(self.on_blocking_task(|this| async move { this.balance(address, block_number) }).await?)
    }

    /// Handler for: `eth_getStorageAt`
    async fn storage_at(
        &self,
        address: Address,
        index: JsonStorageKey,
        block_number: Option<BlockId>,
    ) -> Result<B256> {
        trace!(target: "rpc::eth", ?address, ?block_number, "Serving eth_getStorageAt");
        Ok(self
            .on_blocking_task(|this| async move { this.storage_at(address, index, block_number) })
            .await?)
    }

    /// Handler for: `eth_getTransactionCount`
    async fn transaction_count(
        &self,
        address: Address,
        block_number: Option<BlockId>,
    ) -> Result<U256> {
        trace!(target: "rpc::eth", ?address, ?block_number, "Serving eth_getTransactionCount");
        Ok(self
            .on_blocking_task(
                |this| async move { this.get_transaction_count(address, block_number) },
            )
            .await?)
    }

    /// Handler for: `eth_getCode`
    async fn get_code(&self, address: Address, block_number: Option<BlockId>) -> Result<Bytes> {
        trace!(target: "rpc::eth", ?address, ?block_number, "Serving eth_getCode");
        Ok(self
            .on_blocking_task(|this| async move { this.get_code(address, block_number) })
            .await?)
    }

    /// Handler for: `eth_getHeaderByNumber`
    async fn header_by_number(&self, block_number: BlockNumberOrTag) -> Result<Option<Header>> {
        trace!(target: "rpc::eth", ?block_number, "Serving eth_getHeaderByNumber");
        Ok(EthApi::rpc_block_header(self, block_number).await?)
    }

    /// Handler for: `eth_getHeaderByHash`
    async fn header_by_hash(&self, hash: B256) -> Result<Option<Header>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getHeaderByHash");
        Ok(EthApi::rpc_block_header(self, hash).await?)
    }

    /// Handler for: `eth_call`
    async fn call(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> Result<Bytes> {
        trace!(target: "rpc::eth", ?request, ?block_number, ?state_overrides, ?block_overrides, "Serving eth_call");
        Ok(self
            .call(request, block_number, EvmOverrides::new(state_overrides, block_overrides))
            .await?)
    }

    /// Handler for: `eth_callMany`
    async fn call_many(
        &self,
        bundle: Bundle,
        state_context: Option<StateContext>,
        state_override: Option<StateOverride>,
    ) -> Result<Vec<EthCallResponse>> {
        trace!(target: "rpc::eth", ?bundle, ?state_context, ?state_override, "Serving eth_callMany");
        Ok(EthApi::call_many(self, bundle, state_context, state_override).await?)
    }

    /// Handler for: `eth_createAccessList`
    async fn create_access_list(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
    ) -> Result<AccessListWithGasUsed> {
        trace!(target: "rpc::eth", ?request, ?block_number, "Serving eth_createAccessList");
        let access_list_with_gas_used = self.create_access_list_at(request, block_number).await?;

        Ok(access_list_with_gas_used)
    }

    /// Handler for: `eth_estimateGas`
    async fn estimate_gas(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_override: Option<StateOverride>,
    ) -> Result<U256> {
        trace!(target: "rpc::eth", ?request, ?block_number, "Serving eth_estimateGas");
        Ok(self
            .estimate_gas_at(
                request,
                block_number.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest)),
                state_override,
            )
            .await?)
    }

    /// Handler for: `eth_gasPrice`
    async fn gas_price(&self) -> Result<U256> {
        trace!(target: "rpc::eth", "Serving eth_gasPrice");
        return Ok(EthApi::gas_price(self).await?)
    }

    /// Handler for: `eth_maxPriorityFeePerGas`
    async fn max_priority_fee_per_gas(&self) -> Result<U256> {
        trace!(target: "rpc::eth", "Serving eth_maxPriorityFeePerGas");
        return Ok(EthApi::suggested_priority_fee(self).await?)
    }

    /// Handler for: `eth_blobBaseFee`
    async fn blob_base_fee(&self) -> Result<U256> {
        trace!(target: "rpc::eth", "Serving eth_blobBaseFee");
        return Ok(EthApi::blob_base_fee(self).await?)
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
        block_count: U64HexOrNumber,
        newest_block: BlockNumberOrTag,
        reward_percentiles: Option<Vec<f64>>,
    ) -> Result<FeeHistory> {
        trace!(target: "rpc::eth", ?block_count, ?newest_block, ?reward_percentiles, "Serving eth_feeHistory");
        return Ok(
            EthApi::fee_history(self, block_count.to(), newest_block, reward_percentiles).await?
        )
    }

    /// Handler for: `eth_mining`
    async fn is_mining(&self) -> Result<bool> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for: `eth_hashrate`
    async fn hashrate(&self) -> Result<U256> {
        Ok(U256::ZERO)
    }

    /// Handler for: `eth_getWork`
    async fn get_work(&self) -> Result<Work> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for: `eth_submitHashrate`
    async fn submit_hashrate(&self, _hashrate: U256, _id: B256) -> Result<bool> {
        Ok(false)
    }

    /// Handler for: `eth_submitWork`
    async fn submit_work(&self, _nonce: B64, _pow_hash: B256, _mix_digest: B256) -> Result<bool> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for: `eth_sendTransaction`
    async fn send_transaction(&self, request: TransactionRequest) -> Result<B256> {
        trace!(target: "rpc::eth", ?request, "Serving eth_sendTransaction");
        Ok(EthTransactions::send_transaction(self, request).await?)
    }

    /// Handler for: `eth_sendRawTransaction`
    async fn send_raw_transaction(&self, tx: Bytes) -> Result<B256> {
        trace!(target: "rpc::eth", ?tx, "Serving eth_sendRawTransaction");
        Ok(EthTransactions::send_raw_transaction(self, tx).await?)
    }

    /// Handler for: `eth_sign`
    async fn sign(&self, address: Address, message: Bytes) -> Result<Bytes> {
        trace!(target: "rpc::eth", ?address, ?message, "Serving eth_sign");
        Ok(EthApi::sign(self, address, message).await?)
    }

    /// Handler for: `eth_signTransaction`
    async fn sign_transaction(&self, _transaction: TransactionRequest) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for: `eth_signTypedData`
    async fn sign_typed_data(&self, address: Address, data: Value) -> Result<Bytes> {
        trace!(target: "rpc::eth", ?address, ?data, "Serving eth_signTypedData");
        Ok(EthApi::sign_typed_data(self, data, address)?)
    }

    /// Handler for: `eth_getProof`
    async fn get_proof(
        &self,
        address: Address,
        keys: Vec<JsonStorageKey>,
        block_number: Option<BlockId>,
    ) -> Result<EIP1186AccountProofResponse> {
        trace!(target: "rpc::eth", ?address, ?keys, ?block_number, "Serving eth_getProof");
        let res = EthApi::get_proof(self, address, keys, block_number).await;

        Ok(res.map_err(|e| match e {
            EthApiError::InvalidBlockRange => {
                internal_rpc_err("eth_getProof is unimplemented for historical blocks")
            }
            _ => e.into(),
        })?)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        eth::{
            cache::EthStateCache, gas_oracle::GasPriceOracle, FeeHistoryCache,
            FeeHistoryCacheConfig,
        },
        EthApi,
    };
    use jsonrpsee::types::error::INVALID_PARAMS_CODE;
    use reth_interfaces::test_utils::{generators, generators::Rng};
    use reth_network_api::noop::NoopNetwork;
    use reth_node_ethereum::EthEvmConfig;
    use reth_primitives::{
        basefee::calculate_next_block_base_fee, constants::ETHEREUM_BLOCK_GAS_LIMIT, BaseFeeParams,
        Block, BlockNumberOrTag, Header, TransactionSigned, B256, U256,
    };
    use reth_provider::{
        test_utils::{MockEthProvider, NoopProvider},
        BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, StateProviderFactory,
    };
    use reth_rpc_api::EthApiServer;
    use reth_rpc_types::FeeHistory;
    use reth_tasks::pool::BlockingTaskPool;
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};

    fn build_test_eth_api<
        P: BlockReaderIdExt
            + BlockReader
            + ChainSpecProvider
            + EvmEnvProvider
            + StateProviderFactory
            + Unpin
            + Clone
            + 'static,
    >(
        provider: P,
    ) -> EthApi<P, TestPool, NoopNetwork, EthEvmConfig> {
        let evm_config = EthEvmConfig::default();
        let cache = EthStateCache::spawn(provider.clone(), Default::default(), evm_config);
        let fee_history_cache =
            FeeHistoryCache::new(cache.clone(), FeeHistoryCacheConfig::default());

        EthApi::new(
            provider.clone(),
            testing_pool(),
            NoopNetwork::default(),
            cache.clone(),
            GasPriceOracle::new(provider, Default::default(), cache),
            ETHEREUM_BLOCK_GAS_LIMIT,
            BlockingTaskPool::build().expect("failed to build tracing pool"),
            fee_history_cache,
            evm_config,
        )
    }

    // Function to prepare the EthApi with mock data
    fn prepare_eth_api(
        newest_block: u64,
        mut oldest_block: Option<B256>,
        block_count: u64,
        mock_provider: MockEthProvider,
    ) -> (EthApi<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>, Vec<U256>, Vec<f64>) {
        let mut rng = generators::rng();

        // Build mock data
        let mut gas_used_ratios = Vec::new();
        let mut base_fees_per_gas = Vec::new();
        let mut last_header = None;
        let mut parent_hash = B256::default();

        for i in (0..block_count).rev() {
            let hash = rng.gen();
            let gas_limit: u64 = rng.gen();
            let gas_used: u64 = rng.gen();
            // Note: Generates a u32 to avoid overflows later
            let base_fee_per_gas: Option<u64> = rng.gen::<bool>().then(|| rng.gen::<u32>() as u64);

            let header = Header {
                number: newest_block - i,
                gas_limit,
                gas_used,
                base_fee_per_gas,
                parent_hash,
                ..Default::default()
            };
            last_header = Some(header.clone());
            parent_hash = hash;

            let mut transactions = vec![];
            for _ in 0..100 {
                let random_fee: u128 = rng.gen();

                if let Some(base_fee_per_gas) = header.base_fee_per_gas {
                    let transaction = TransactionSigned {
                        transaction: reth_primitives::Transaction::Eip1559(
                            reth_primitives::TxEip1559 {
                                max_priority_fee_per_gas: random_fee,
                                max_fee_per_gas: random_fee + base_fee_per_gas as u128,
                                ..Default::default()
                            },
                        ),
                        ..Default::default()
                    };

                    transactions.push(transaction);
                } else {
                    let transaction = TransactionSigned {
                        transaction: reth_primitives::Transaction::Legacy(
                            reth_primitives::TxLegacy { ..Default::default() },
                        ),
                        ..Default::default()
                    };

                    transactions.push(transaction);
                }
            }

            mock_provider.add_block(
                hash,
                Block { header: header.clone(), body: transactions, ..Default::default() },
            );
            mock_provider.add_header(hash, header);

            oldest_block.get_or_insert(hash);
            gas_used_ratios.push(gas_used as f64 / gas_limit as f64);
            base_fees_per_gas
                .push(base_fee_per_gas.map(|fee| U256::try_from(fee).unwrap()).unwrap_or_default());
        }

        // Add final base fee (for the next block outside of the request)
        let last_header = last_header.unwrap();
        base_fees_per_gas.push(U256::from(calculate_next_block_base_fee(
            last_header.gas_used,
            last_header.gas_limit,
            last_header.base_fee_per_gas.unwrap_or_default(),
            BaseFeeParams::ethereum(),
        )));

        let eth_api = build_test_eth_api(mock_provider);

        (eth_api, base_fees_per_gas, gas_used_ratios)
    }

    /// Invalid block range
    #[tokio::test]
    async fn test_fee_history_empty() {
        let response = <EthApi<_, _, _, _> as EthApiServer>::fee_history(
            &build_test_eth_api(NoopProvider::default()),
            1.into(),
            BlockNumberOrTag::Latest,
            None,
        )
        .await;
        assert!(response.is_err());
        let error_object = response.unwrap_err();
        assert_eq!(error_object.code(), INVALID_PARAMS_CODE);
    }

    #[tokio::test]
    /// Invalid block range (request is before genesis)
    async fn test_fee_history_invalid_block_range_before_genesis() {
        let block_count = 10;
        let newest_block = 1337;
        let oldest_block = None;

        let (eth_api, _, _) =
            prepare_eth_api(newest_block, oldest_block, block_count, MockEthProvider::default());

        let response = <EthApi<_, _, _, _> as EthApiServer>::fee_history(
            &eth_api,
            (newest_block + 1).into(),
            newest_block.into(),
            Some(vec![10.0]),
        )
        .await;

        assert!(response.is_err());
        let error_object = response.unwrap_err();
        assert_eq!(error_object.code(), INVALID_PARAMS_CODE);
    }

    #[tokio::test]
    /// Invalid block range (request is in the future)
    async fn test_fee_history_invalid_block_range_in_future() {
        let block_count = 10;
        let newest_block = 1337;
        let oldest_block = None;

        let (eth_api, _, _) =
            prepare_eth_api(newest_block, oldest_block, block_count, MockEthProvider::default());

        let response = <EthApi<_, _, _, _> as EthApiServer>::fee_history(
            &eth_api,
            1.into(),
            (newest_block + 1000).into(),
            Some(vec![10.0]),
        )
        .await;

        assert!(response.is_err());
        let error_object = response.unwrap_err();
        assert_eq!(error_object.code(), INVALID_PARAMS_CODE);
    }

    #[tokio::test]
    /// Requesting no block should result in a default response
    async fn test_fee_history_no_block_requested() {
        let block_count = 10;
        let newest_block = 1337;
        let oldest_block = None;

        let (eth_api, _, _) =
            prepare_eth_api(newest_block, oldest_block, block_count, MockEthProvider::default());

        let response = <EthApi<_, _, _, _> as EthApiServer>::fee_history(
            &eth_api,
            0.into(),
            newest_block.into(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            response,
            FeeHistory::default(),
            "none: requesting no block should yield a default response"
        );
    }

    #[tokio::test]
    /// Requesting a single block should return 1 block (+ base fee for the next block over)
    async fn test_fee_history_single_block() {
        let block_count = 10;
        let newest_block = 1337;
        let oldest_block = None;

        let (eth_api, base_fees_per_gas, gas_used_ratios) =
            prepare_eth_api(newest_block, oldest_block, block_count, MockEthProvider::default());

        let fee_history = eth_api.fee_history(1, newest_block.into(), None).await.unwrap();
        assert_eq!(
            &fee_history.base_fee_per_gas,
            &base_fees_per_gas[base_fees_per_gas.len() - 2..],
            "one: base fee per gas is incorrect"
        );
        assert_eq!(
            fee_history.base_fee_per_gas.len(),
            2,
            "one: should return base fee of the next block as well"
        );
        assert_eq!(
            &fee_history.gas_used_ratio,
            &gas_used_ratios[gas_used_ratios.len() - 1..],
            "one: gas used ratio is incorrect"
        );
        assert_eq!(
            fee_history.oldest_block,
            U256::from(newest_block),
            "one: oldest block is incorrect"
        );
        assert!(
            fee_history.reward.is_none(),
            "one: no percentiles were requested, so there should be no rewards result"
        );
    }

    #[tokio::test]
    /// Requesting all blocks should be ok
    async fn test_fee_history_all_blocks() {
        let block_count = 10;
        let newest_block = 1337;
        let oldest_block = None;

        let (eth_api, base_fees_per_gas, gas_used_ratios) =
            prepare_eth_api(newest_block, oldest_block, block_count, MockEthProvider::default());

        let fee_history =
            eth_api.fee_history(block_count, newest_block.into(), None).await.unwrap();

        assert_eq!(
            &fee_history.base_fee_per_gas, &base_fees_per_gas,
            "all: base fee per gas is incorrect"
        );
        assert_eq!(
            fee_history.base_fee_per_gas.len() as u64,
            block_count + 1,
            "all: should return base fee of the next block as well"
        );
        assert_eq!(
            &fee_history.gas_used_ratio, &gas_used_ratios,
            "all: gas used ratio is incorrect"
        );
        assert_eq!(
            fee_history.oldest_block,
            U256::from(newest_block - block_count + 1),
            "all: oldest block is incorrect"
        );
        assert!(
            fee_history.reward.is_none(),
            "all: no percentiles were requested, so there should be no rewards result"
        );
    }
}
