//! Implementation of the [`jsonrpsee`] generated [`reth_rpc_api::EthApiServer`] trait
//! Handles RPC requests for the `eth_` namespace.

use super::EthApiSpec;
use crate::{
    eth::{
        api::{EthApi, EthTransactions},
        revm_utils::EvmOverrides,
    },
    result::{internal_rpc_err, ToRpcResult},
};
use jsonrpsee::core::RpcResult as Result;
use reth_network_api::NetworkInfo;
use reth_primitives::{
    serde_helper::JsonStorageKey, AccessListWithGasUsed, Address, BlockId, BlockNumberOrTag, Bytes,
    H256, H64, U256, U64,
};
use reth_provider::{
    BlockIdReader, BlockReader, BlockReaderIdExt, EvmEnvProvider, HeaderProvider,
    StateProviderFactory,
};
use reth_rpc_api::EthApiServer;
use reth_rpc_types::{
    state::StateOverride, BlockOverrides, CallRequest, EIP1186AccountProofResponse, FeeHistory,
    Index, RichBlock, SyncStatus, TransactionReceipt, TransactionRequest, Work,
};
use reth_transaction_pool::TransactionPool;
use serde_json::Value;
use tracing::trace;

#[async_trait::async_trait]
impl<Provider, Pool, Network> EthApiServer for EthApi<Provider, Pool, Network>
where
    Self: EthApiSpec + EthTransactions,
    Pool: TransactionPool + 'static,
    Provider: BlockReader
        + BlockIdReader
        + BlockReaderIdExt
        + HeaderProvider
        + StateProviderFactory
        + EvmEnvProvider
        + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
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
    async fn accounts(&self) -> Result<Vec<Address>> {
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
    async fn block_by_hash(&self, hash: H256, full: bool) -> Result<Option<RichBlock>> {
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
    async fn block_transaction_count_by_hash(&self, hash: H256) -> Result<Option<U256>> {
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
    async fn block_uncles_count_by_hash(&self, hash: H256) -> Result<Option<U256>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getUncleCountByBlockHash");
        Ok(EthApi::ommers(self, hash)?.map(|ommers| U256::from(ommers.len())))
    }

    /// Handler for: `eth_getUncleCountByBlockNumber`
    async fn block_uncles_count_by_number(&self, number: BlockNumberOrTag) -> Result<Option<U256>> {
        trace!(target: "rpc::eth", ?number, "Serving eth_getUncleCountByBlockNumber");
        Ok(EthApi::ommers(self, number)?.map(|ommers| U256::from(ommers.len())))
    }

    /// Handler for: `eth_getUncleByBlockHashAndIndex`
    async fn uncle_by_block_hash_and_index(
        &self,
        hash: H256,
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

    /// Handler for: `eth_getTransactionByHash`
    async fn transaction_by_hash(&self, hash: H256) -> Result<Option<reth_rpc_types::Transaction>> {
        trace!(target: "rpc::eth", ?hash, "Serving eth_getTransactionByHash");
        Ok(EthTransactions::transaction_by_hash(self, hash).await?.map(Into::into))
    }

    /// Handler for: `eth_getTransactionByBlockHashAndIndex`
    async fn transaction_by_block_hash_and_index(
        &self,
        hash: H256,
        index: Index,
    ) -> Result<Option<reth_rpc_types::Transaction>> {
        trace!(target: "rpc::eth", ?hash, ?index, "Serving eth_getTransactionByBlockHashAndIndex");
        Ok(EthApi::transaction_by_block_and_tx_index(self, hash, index).await?)
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
    async fn transaction_receipt(&self, hash: H256) -> Result<Option<TransactionReceipt>> {
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
    ) -> Result<H256> {
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

    /// Handler for: `eth_call`
    async fn call(
        &self,
        request: CallRequest,
        block_number: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> Result<Bytes> {
        trace!(target: "rpc::eth", ?request, ?block_number, ?state_overrides, ?block_overrides, "Serving eth_call");
        Ok(self
            .on_blocking_task(|this| async move {
                this.call(
                    request,
                    block_number,
                    EvmOverrides::new(state_overrides, block_overrides),
                )
                .await
            })
            .await?)
    }

    /// Handler for: `eth_createAccessList`
    async fn create_access_list(
        &self,
        mut request: CallRequest,
        block_number: Option<BlockId>,
    ) -> Result<AccessListWithGasUsed> {
        trace!(target: "rpc::eth", ?request, ?block_number, "Serving eth_createAccessList");
        Ok(self
            .on_blocking_task(|this| async move {
                let block_id = block_number.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));
                let access_list = this.create_access_list_at(request.clone(), block_number).await?;
                request.access_list = Some(access_list.clone());
                let gas_used = this.estimate_gas_at(request, block_id).await?;
                Ok(AccessListWithGasUsed { access_list, gas_used })
            })
            .await?)
    }

    /// Handler for: `eth_estimateGas`
    async fn estimate_gas(
        &self,
        request: CallRequest,
        block_number: Option<BlockId>,
    ) -> Result<U256> {
        trace!(target: "rpc::eth", ?request, ?block_number, "Serving eth_estimateGas");
        Ok(self
            .on_blocking_task(|this| async move {
                this.estimate_gas_at(
                    request,
                    block_number.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest)),
                )
                .await
            })
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
        newest_block: BlockId,
        reward_percentiles: Option<Vec<f64>>,
    ) -> Result<FeeHistory> {
        trace!(target: "rpc::eth", ?block_count, ?newest_block, ?reward_percentiles, "Serving eth_feeHistory");
        return Ok(EthApi::fee_history(self, block_count.as_u64(), newest_block, reward_percentiles)
            .await?)
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
    async fn submit_hashrate(&self, _hashrate: U256, _id: H256) -> Result<bool> {
        Ok(false)
    }

    /// Handler for: `eth_submitWork`
    async fn submit_work(&self, _nonce: H64, _pow_hash: H256, _mix_digest: H256) -> Result<bool> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for: `eth_sendTransaction`
    async fn send_transaction(&self, request: TransactionRequest) -> Result<H256> {
        trace!(target: "rpc::eth", ?request, "Serving eth_sendTransaction");
        Ok(EthTransactions::send_transaction(self, request).await?)
    }

    /// Handler for: `eth_sendRawTransaction`
    async fn send_raw_transaction(&self, tx: Bytes) -> Result<H256> {
        trace!(target: "rpc::eth", ?tx, "Serving eth_sendRawTransaction");
        Ok(EthTransactions::send_raw_transaction(self, tx).await?)
    }

    /// Handler for: `eth_sign`
    async fn sign(&self, address: Address, message: Bytes) -> Result<Bytes> {
        trace!(target: "rpc::eth", ?address, ?message, "Serving eth_sign");
        Ok(EthApi::sign(self, address, message).await?)
    }

    /// Handler for: `eth_signTransaction`
    async fn sign_transaction(&self, _transaction: CallRequest) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for: `eth_signTypedData`
    async fn sign_typed_data(&self, address: Address, data: Value) -> Result<Bytes> {
        trace!(target: "rpc::eth", ?address, ?data, "Serving eth_signTypedData");
        Ok(EthApi::sign_typed_data(self, data, address).await?)
    }

    /// Handler for: `eth_getProof`
    async fn get_proof(
        &self,
        _address: Address,
        _keys: Vec<JsonStorageKey>,
        _block_number: Option<BlockId>,
    ) -> Result<EIP1186AccountProofResponse> {
        // TODO: uncomment when implemented
        // trace!(target: "rpc::eth", ?address, ?keys, ?block_number, "Serving eth_getProof");
        // let res = EthApi::get_proof(self, address, keys, block_number);

        // Ok(res.map_err(|e| match e {
        //     EthApiError::InvalidBlockRange => {
        //         internal_rpc_err("eth_getProof is unimplemented for historical blocks")
        //     }
        //     _ => e.into(),
        // })?)
        Err(internal_rpc_err("unimplemented"))
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        eth::{cache::EthStateCache, gas_oracle::GasPriceOracle},
        EthApi,
    };
    use jsonrpsee::types::error::INVALID_PARAMS_CODE;
    use rand::random;
    use reth_network_api::test_utils::NoopNetwork;
    use reth_primitives::{Block, BlockNumberOrTag, Header, TransactionSigned, H256, U256};
    use reth_provider::test_utils::{MockEthProvider, NoopProvider};
    use reth_rpc_api::EthApiServer;
    use reth_transaction_pool::test_utils::testing_pool;

    #[tokio::test]
    /// Handler for: `eth_test_fee_history`
    async fn test_fee_history() {
        let cache = EthStateCache::spawn(NoopProvider::default(), Default::default());
        let eth_api = EthApi::new(
            NoopProvider::default(),
            testing_pool(),
            NoopNetwork,
            cache.clone(),
            GasPriceOracle::new(NoopProvider::default(), Default::default(), cache),
        );

        let response = <EthApi<_, _, _> as EthApiServer>::fee_history(
            &eth_api,
            1.into(),
            BlockNumberOrTag::Latest.into(),
            None,
        )
        .await;
        assert!(response.is_err());
        let error_object = response.unwrap_err();
        assert_eq!(error_object.code(), INVALID_PARAMS_CODE);

        let block_count = 10;
        let newest_block = 1337;

        let mut oldest_block = None;
        let mut gas_used_ratios = Vec::new();
        let mut base_fees_per_gas = Vec::new();

        let mock_provider = MockEthProvider::default();

        for i in (0..=block_count).rev() {
            let hash = H256::random();
            let gas_limit: u64 = random();
            let gas_used: u64 = random();
            let base_fee_per_gas: Option<u64> = random::<bool>().then(random);

            let header = Header {
                number: newest_block - i,
                gas_limit,
                gas_used,
                base_fee_per_gas,
                ..Default::default()
            };

            let mut transactions = vec![];
            for _ in 0..100 {
                let random_fee: u128 = random();

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

        gas_used_ratios.pop();

        let cache = EthStateCache::spawn(mock_provider.clone(), Default::default());
        let eth_api = EthApi::new(
            mock_provider.clone(),
            testing_pool(),
            NoopNetwork,
            cache.clone(),
            GasPriceOracle::new(mock_provider, Default::default(), cache.clone()),
        );

        let response = <EthApi<_, _, _> as EthApiServer>::fee_history(
            &eth_api,
            (newest_block + 1).into(),
            newest_block.into(),
            Some(vec![10.0]),
        )
        .await;
        assert!(response.is_err());
        let error_object = response.unwrap_err();
        assert_eq!(error_object.code(), INVALID_PARAMS_CODE);

        // newest_block is finalized
        let fee_history =
            eth_api.fee_history(block_count, (newest_block - 1).into(), None).await.unwrap();

        assert_eq!(fee_history.base_fee_per_gas, base_fees_per_gas);
        assert_eq!(fee_history.gas_used_ratio, gas_used_ratios);
        assert_eq!(fee_history.oldest_block, U256::from(newest_block - block_count));

        // newest_block is pending
        let fee_history =
            eth_api.fee_history(block_count, (newest_block - 1).into(), None).await.unwrap();

        assert_eq!(fee_history.base_fee_per_gas, base_fees_per_gas);
        assert_eq!(fee_history.gas_used_ratio, gas_used_ratios);
        assert_eq!(fee_history.oldest_block, U256::from(newest_block - block_count));
    }
}
