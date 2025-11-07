//! X Layer: Legacy RPC support for routing historical data to legacy endpoints.

use alloy_primitives::{Address, BlockHash, BlockNumber, Bytes, TxHash, B256, U256};
use alloy_rpc_types_eth::{
    Block, BlockId, BlockNumberOrTag, Filter, Index, Log, Transaction, TransactionReceipt,
};
use alloy_serde::JsonStorageKey;
use jsonrpsee::{
    core::client::ClientT,
    http_client::{HttpClient, HttpClientBuilder},
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for legacy RPC routing.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LegacyRpcConfig {
    /// The block number before which requests should be routed to the legacy RPC endpoint.
    pub cutoff_block: BlockNumber,
    /// The URL of the legacy RPC endpoint.
    pub endpoint: String,
    /// Request timeout for legacy RPC calls.
    #[serde(with = "humantime_serde")]
    pub timeout: Duration,
}

impl LegacyRpcConfig {
    /// Create a new legacy RPC configuration.
    pub fn new(cutoff_block: BlockNumber, endpoint: String, timeout: Duration) -> Self {
        Self { cutoff_block, endpoint, timeout }
    }
}

/// HTTP client for interacting with legacy RPC endpoint.
#[derive(Debug, Clone)]
pub struct LegacyRpcClient {
    client: HttpClient,
    cutoff_block: BlockNumber,
}

impl LegacyRpcClient {
    /// Create a new legacy RPC client from configuration.
    pub fn from_config(config: &LegacyRpcConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let client = HttpClientBuilder::default()
            .request_timeout(config.timeout)
            .build(&config.endpoint)?;

        Ok(Self {
            client,
            cutoff_block: config.cutoff_block,
        })
    }

    /// Get the cutoff block number.
    #[inline]
    pub fn cutoff_block(&self) -> BlockNumber {
        self.cutoff_block
    }

    /// Helper to convert jsonrpsee error to boxed error
    #[inline]
    fn to_box_err<T>(
        result: Result<T, jsonrpsee::core::ClientError>,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>> {
        result.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    /// Forward eth_getBlockByNumber to legacy RPC.
    pub async fn get_block_by_number(
        &self,
        block_number: BlockNumberOrTag,
        full: bool,
    ) -> Result<Option<Block>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getBlockByNumber", (block_number, full)).await)
    }

    /// Forward eth_getBlockByHash to legacy RPC.
    pub async fn get_block_by_hash(
        &self,
        hash: BlockHash,
        full: bool,
    ) -> Result<Option<Block>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getBlockByHash", (hash, full)).await)
    }

    /// Forward eth_getTransactionByHash to legacy RPC.
    pub async fn get_transaction_by_hash(
        &self,
        hash: TxHash,
    ) -> Result<Option<Transaction>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getTransactionByHash", (hash,)).await)
    }

    /// Forward eth_getTransactionReceipt to legacy RPC.
    pub async fn get_transaction_receipt(
        &self,
        hash: TxHash,
    ) -> Result<Option<TransactionReceipt>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getTransactionReceipt", (hash,)).await)
    }

    /// Forward eth_getLogs to legacy RPC.
    pub async fn get_logs(
        &self,
        filter: Filter,
    ) -> Result<Vec<Log>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getLogs", (filter,)).await)
    }

    /// Forward eth_getBlockTransactionCountByNumber to legacy RPC.
    pub async fn get_block_transaction_count_by_number(
        &self,
        block_number: BlockNumberOrTag,
    ) -> Result<Option<U256>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getBlockTransactionCountByNumber", (block_number,)).await)
    }

    /// Forward eth_getBlockTransactionCountByHash to legacy RPC.
    pub async fn get_block_transaction_count_by_hash(
        &self,
        hash: BlockHash,
    ) -> Result<Option<U256>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getBlockTransactionCountByHash", (hash,)).await)
    }

    /// Forward eth_getBalance to legacy RPC.
    pub async fn get_balance(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> Result<U256, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getBalance", (address, block_id)).await)
    }

    /// Forward eth_getCode to legacy RPC.
    pub async fn get_code(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> Result<Bytes, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getCode", (address, block_id)).await)
    }

    /// Forward eth_getStorageAt to legacy RPC.
    pub async fn get_storage_at(
        &self,
        address: Address,
        index: JsonStorageKey,
        block_id: Option<BlockId>,
    ) -> Result<B256, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getStorageAt", (address, index, block_id)).await)
    }

    /// Forward eth_getTransactionCount to legacy RPC.
    pub async fn get_transaction_count(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> Result<U256, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getTransactionCount", (address, block_id)).await)
    }

    /// Forward eth_getTransactionByBlockHashAndIndex to legacy RPC.
    pub async fn get_transaction_by_block_hash_and_index(
        &self,
        hash: BlockHash,
        index: Index,
    ) -> Result<Option<Transaction>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getTransactionByBlockHashAndIndex", (hash, index)).await)
    }

    /// Forward eth_getTransactionByBlockNumberAndIndex to legacy RPC.
    pub async fn get_transaction_by_block_number_and_index(
        &self,
        block_number: BlockNumberOrTag,
        index: Index,
    ) -> Result<Option<Transaction>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getTransactionByBlockNumberAndIndex", (block_number, index)).await)
    }

    /// Forward eth_getBlockReceipts to legacy RPC.
    pub async fn get_block_receipts(
        &self,
        block_id: BlockId,
    ) -> Result<Option<Vec<TransactionReceipt>>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getBlockReceipts", (block_id,)).await)
    }

    /// Forward eth_getHeaderByNumber to legacy RPC.
    pub async fn get_header_by_number(
        &self,
        block_number: BlockNumberOrTag,
    ) -> Result<Option<alloy_rpc_types_eth::Header>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getHeaderByNumber", (block_number,)).await)
    }

    /// Forward eth_getHeaderByHash to legacy RPC.
    pub async fn get_header_by_hash(
        &self,
        hash: BlockHash,
    ) -> Result<Option<alloy_rpc_types_eth::Header>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getHeaderByHash", (hash,)).await)
    }

    /// Forward eth_getRawTransactionByHash to legacy RPC.
    pub async fn get_raw_transaction_by_hash(
        &self,
        hash: TxHash,
    ) -> Result<Option<Bytes>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getRawTransactionByHash", (hash,)).await)
    }

    /// Forward eth_getRawTransactionByBlockHashAndIndex to legacy RPC.
    pub async fn get_raw_transaction_by_block_hash_and_index(
        &self,
        hash: BlockHash,
        index: Index,
    ) -> Result<Option<Bytes>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getRawTransactionByBlockHashAndIndex", (hash, index)).await)
    }

    /// Forward eth_getRawTransactionByBlockNumberAndIndex to legacy RPC.
    pub async fn get_raw_transaction_by_block_number_and_index(
        &self,
        block_number: BlockNumberOrTag,
        index: Index,
    ) -> Result<Option<Bytes>, Box<dyn std::error::Error + Send + Sync>> {
        Self::to_box_err(self.client.request("eth_getRawTransactionByBlockNumberAndIndex", (block_number, index)).await)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_rpc_config() {
        let config = LegacyRpcConfig::new(
            1000000,
            "http://legacy:8545".to_string(),
            std::time::Duration::from_secs(30),
        );
        assert_eq!(config.cutoff_block, 1000000);
        assert_eq!(config.endpoint, "http://legacy:8545");
    }
}
