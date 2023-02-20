//! Implementation of the [`jsonrpsee`] generated [`reth_rpc_api::EthApiServer`] trait
//! Handles RPC requests for the `eth_` namespace.

use super::EthApiSpec;
use crate::{
    eth::{api::EthApi, error::EthApiError},
    result::{internal_rpc_err, ToRpcResult},
};
use jsonrpsee::core::RpcResult as Result;
use reth_primitives::{
    rpc::transaction::eip2930::AccessListWithGasUsed, Address, BlockId, BlockNumberOrTag, Bytes,
    Header, H256, H64, U256, U64,
};
use reth_provider::{BlockProvider, HeaderProvider, StateProviderFactory};
use reth_rpc_api::EthApiServer;
use reth_rpc_types::{
    CallRequest, EIP1186AccountProofResponse, FeeHistory, FeeHistoryCacheItem, Index, RichBlock,
    SyncStatus, TransactionReceipt, TransactionRequest, Work,
};
use reth_transaction_pool::TransactionPool;
use serde_json::Value;
use std::collections::BTreeMap;

#[async_trait::async_trait]
impl<Client, Pool, Network> EthApiServer for EthApi<Client, Pool, Network>
where
    Self: EthApiSpec,
    Pool: TransactionPool + 'static,
    Client: BlockProvider + HeaderProvider + StateProviderFactory + 'static,
    Network: 'static,
{
    async fn protocol_version(&self) -> Result<U64> {
        EthApiSpec::protocol_version(self).await.to_rpc_result()
    }

    fn syncing(&self) -> Result<SyncStatus> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn author(&self) -> Result<Address> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn accounts(&self) -> Result<Vec<Address>> {
        Ok(EthApiSpec::accounts(self))
    }

    fn block_number(&self) -> Result<U256> {
        Ok(U256::from(
            EthApiSpec::chain_info(self).with_message("failed to read chain info")?.best_number,
        ))
    }

    async fn chain_id(&self) -> Result<Option<U64>> {
        Ok(Some(EthApiSpec::chain_id(self)))
    }

    async fn block_by_hash(&self, hash: H256, full: bool) -> Result<Option<RichBlock>> {
        Ok(EthApi::block(self, hash, full).await?)
    }

    async fn block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> Result<Option<RichBlock>> {
        Ok(EthApi::block(self, number, full).await?)
    }

    async fn block_transaction_count_by_hash(&self, _hash: H256) -> Result<Option<U256>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn block_transaction_count_by_number(
        &self,
        _number: BlockNumberOrTag,
    ) -> Result<Option<U256>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn block_uncles_count_by_hash(&self, _hash: H256) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn block_uncles_count_by_number(&self, _number: BlockNumberOrTag) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn uncle_by_block_hash_and_index(
        &self,
        _hash: H256,
        _index: Index,
    ) -> Result<Option<RichBlock>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn uncle_by_block_number_and_index(
        &self,
        _number: BlockNumberOrTag,
        _index: Index,
    ) -> Result<Option<RichBlock>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn transaction_by_hash(
        &self,
        _hash: H256,
    ) -> Result<Option<reth_rpc_types::Transaction>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn transaction_by_block_hash_and_index(
        &self,
        _hash: H256,
        _index: Index,
    ) -> Result<Option<reth_rpc_types::Transaction>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn transaction_by_block_number_and_index(
        &self,
        _number: BlockNumberOrTag,
        _index: Index,
    ) -> Result<Option<reth_rpc_types::Transaction>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn transaction_receipt(&self, _hash: H256) -> Result<Option<TransactionReceipt>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn balance(&self, address: Address, block_number: Option<BlockId>) -> Result<U256> {
        Ok(EthApi::balance(self, address, block_number)?)
    }

    async fn storage_at(
        &self,
        address: Address,
        index: U256,
        block_number: Option<BlockId>,
    ) -> Result<H256> {
        Ok(EthApi::storage_at(self, address, index, block_number)?)
    }

    async fn transaction_count(
        &self,
        address: Address,
        block_number: Option<BlockId>,
    ) -> Result<U256> {
        Ok(EthApi::get_transaction_count(self, address, block_number)?)
    }

    async fn get_code(&self, address: Address, block_number: Option<BlockId>) -> Result<Bytes> {
        Ok(EthApi::get_code(self, address, block_number)?)
    }

    async fn call(&self, _request: CallRequest, _block_number: Option<BlockId>) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn create_access_list(
        &self,
        _request: CallRequest,
        _block_number: Option<BlockId>,
    ) -> Result<AccessListWithGasUsed> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn estimate_gas(
        &self,
        _request: CallRequest,
        _block_number: Option<BlockId>,
    ) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn gas_price(&self) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    // FeeHistory is calculated based on lazy evaluation of fees for historical blocks, and further
    // caching of it in the LRU cache.
    // When new RPC call is executed, the cache gets locked, we check it for the historical fees
    // according to the requested block range, and fill any cache misses (in both RPC response
    // and cache itself) with the actual data queried from the database.
    // To minimize the number of database seeks required to query the missing data, we calculate the
    // first non-cached block number and last non-cached block number. After that, we query this
    // range of consecutive blocks from the database.
    async fn fee_history(
        &self,
        block_count: U64,
        newest_block: BlockId,
        _reward_percentiles: Option<Vec<f64>>,
    ) -> Result<FeeHistory> {
        let block_count = block_count.as_u64();

        if block_count == 0 {
            return Ok(FeeHistory::default())
        }

        let Some(end_block) = self.inner.client.block_number_for_id(newest_block).to_rpc_result()? else { return Err(EthApiError::UnknownBlockNumber.into())};

        if end_block < block_count {
            return Err(EthApiError::InvalidBlockRange.into())
        }

        let start_block = end_block - block_count;

        let mut fee_history_cache = self.fee_history_cache.0.lock().await;

        // Sorted map that's populated in two rounds:
        // 1. Cache entries until first non-cached block
        // 2. Database query from the first non-cached block
        let mut fee_history_cache_items = BTreeMap::new();

        let mut first_non_cached_block = None;
        let mut last_non_cached_block = None;
        for block in start_block..=end_block {
            // Check if block exists in cache, and move it to the head of the list if so
            if let Some(fee_history_cache_item) = fee_history_cache.get(&block) {
                fee_history_cache_items.insert(block, fee_history_cache_item.clone());
            } else {
                // If block doesn't exist in cache, set it as a first non-cached block to query it
                // from the database
                first_non_cached_block.get_or_insert(block);
                // And last non-cached block, so we could query the database until we reach it
                last_non_cached_block = Some(block);
            }
        }

        // If we had any cache misses, query the database starting with the first non-cached block
        // and ending with the last
        if let (Some(start_block), Some(end_block)) =
            (first_non_cached_block, last_non_cached_block)
        {
            let headers: Vec<Header> =
                self.inner.client.headers_range(start_block..=end_block).to_rpc_result()?;

            // We should receive exactly the amount of blocks missing from the cache
            if headers.len() != (end_block - start_block + 1) as usize {
                return Err(EthApiError::InvalidBlockRange.into())
            }

            for header in headers {
                let base_fee_per_gas = header.base_fee_per_gas.
                        unwrap_or_default(). // Zero for pre-EIP-1559 blocks
                        try_into().unwrap(); // u64 -> U256 won't fail
                let gas_used_ratio = header.gas_used as f64 / header.gas_limit as f64;

                let fee_history_cache_item = FeeHistoryCacheItem {
                    hash: None,
                    base_fee_per_gas,
                    gas_used_ratio,
                    reward: None, // TODO: calculate rewards per transaction
                };

                // Insert missing cache entries in the map for further response composition from it
                fee_history_cache_items.insert(header.number, fee_history_cache_item.clone());
                // And populate the cache with new entries
                fee_history_cache.push(header.number, fee_history_cache_item);
            }
        }

        let oldest_block_hash =
            self.inner.client.block_hash(start_block.try_into().unwrap()).to_rpc_result()?.unwrap();

        fee_history_cache_items.get_mut(&start_block).unwrap().hash = Some(oldest_block_hash);
        fee_history_cache.get_mut(&start_block).unwrap().hash = Some(oldest_block_hash);

        // `fee_history_cache_items` now contains full requested block range (populated from both
        // cache and database), so we can iterate over it in order and populate the response fields
        Ok(FeeHistory {
            base_fee_per_gas: fee_history_cache_items
                .values()
                .map(|item| item.base_fee_per_gas)
                .collect(),
            gas_used_ratio: fee_history_cache_items
                .values()
                .map(|item| item.gas_used_ratio)
                .collect(),
            oldest_block: U256::from_be_bytes(oldest_block_hash.0),
            reward: None,
        })
    }

    async fn max_priority_fee_per_gas(&self) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn is_mining(&self) -> Result<bool> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn hashrate(&self) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn get_work(&self) -> Result<Work> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn submit_hashrate(&self, _hashrate: U256, _id: H256) -> Result<bool> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn submit_work(&self, _nonce: H64, _pow_hash: H256, _mix_digest: H256) -> Result<bool> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn send_transaction(&self, _request: TransactionRequest) -> Result<H256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn send_raw_transaction(&self, tx: Bytes) -> Result<H256> {
        Ok(EthApi::send_raw_transaction(self, tx).await?)
    }

    async fn sign(&self, _address: Address, _message: Bytes) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn sign_transaction(&self, _transaction: CallRequest) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn sign_typed_data(&self, _address: Address, _data: Value) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn get_proof(
        &self,
        _address: Address,
        _keys: Vec<H256>,
        _block_number: Option<BlockId>,
    ) -> Result<EIP1186AccountProofResponse> {
        Err(internal_rpc_err("unimplemented"))
    }
}

#[cfg(test)]
mod tests {
    use jsonrpsee::{
        core::{error::Error as RpcError, RpcResult},
        types::error::{CallError, INVALID_PARAMS_CODE},
    };
    use rand::random;
    use reth_network_api::test_utils::NoopNetwork;
    use reth_primitives::{Block, BlockNumberOrTag, Header, H256, U256};
    use reth_provider::test_utils::{MockEthProvider, NoopProvider};
    use reth_rpc_api::EthApiServer;
    use reth_transaction_pool::test_utils::testing_pool;

    use crate::EthApi;

    #[tokio::test]
    async fn test_fee_history() {
        let eth_api = EthApi::new(NoopProvider::default(), testing_pool(), NoopNetwork::default());

        let response = eth_api.fee_history(1.into(), BlockNumberOrTag::Latest.into(), None).await;
        assert!(matches!(response, RpcResult::Err(RpcError::Call(CallError::Custom(_)))));
        let Err(RpcError::Call(CallError::Custom(error_object))) = response else { unreachable!() };
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
            let base_fee_per_gas: Option<u64> =
                if random::<bool>() { Some(random()) } else { None };

            let header = Header {
                number: newest_block - i,
                gas_limit,
                gas_used,
                base_fee_per_gas,
                ..Default::default()
            };

            mock_provider.add_block(hash, Block { header: header.clone(), ..Default::default() });
            mock_provider.add_header(hash, header);

            oldest_block.get_or_insert(hash);
            gas_used_ratios.push(gas_used as f64 / gas_limit as f64);
            base_fees_per_gas
                .push(base_fee_per_gas.map(|fee| U256::try_from(fee).unwrap()).unwrap_or_default());
        }

        let eth_api = EthApi::new(mock_provider, testing_pool(), NoopNetwork::default());

        let response =
            eth_api.fee_history((newest_block + 1).into(), newest_block.into(), None).await;
        assert!(matches!(response, RpcResult::Err(RpcError::Call(CallError::Custom(_)))));
        let Err(RpcError::Call(CallError::Custom(error_object))) = response else { unreachable!() };
        assert_eq!(error_object.code(), INVALID_PARAMS_CODE);

        let fee_history =
            eth_api.fee_history(block_count.into(), newest_block.into(), None).await.unwrap();

        assert_eq!(fee_history.base_fee_per_gas, base_fees_per_gas);
        assert_eq!(fee_history.gas_used_ratio, gas_used_ratios);
        assert_eq!(fee_history.oldest_block, U256::from_be_bytes(oldest_block.unwrap().0));
    }
}
