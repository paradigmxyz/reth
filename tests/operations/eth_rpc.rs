use alloy_primitives::U256;
use eyre::Result;
use jsonrpsee::{core::client::ClientT, http_client::HttpClient};
use serde_json::{json, Value};
use std::time::Duration;

const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Block identifier for RPC calls - can be either a number or hash
#[derive(Debug, Clone)]
pub enum BlockId {
    /// Block number as u64
    Number(u64),
    /// Block hash as string
    Hash(String),
    /// Latest tag
    Latest,
    /// Pending tag
    Pending,
}

impl BlockId {
    /// Convert BlockId to a string suitable for RPC calls
    pub fn to_rpc_param(&self) -> String {
        match self {
            BlockId::Number(num) => format!("0x{:x}", num),
            BlockId::Hash(hash) => hash.clone(),
            BlockId::Latest => "latest".to_string(),
            BlockId::Pending => "pending".to_string(),
        }
    }
}

fn from_hex_to_u64(hex_str: &str) -> Result<u64> {
    u64::from_str_radix(hex_str.trim_start_matches("0x"), 16).map_err(|e| eyre::eyre!(e))
}

fn from_hex_to_u256(hex_str: &str) -> Result<U256> {
    U256::from_str_radix(hex_str.trim_start_matches("0x"), 16).map_err(|e| eyre::eyre!(e))
}

/// For eth_chainId
pub async fn eth_chain_id(client_rpc: &HttpClient) -> Result<u64> {
    let result: String = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_chainId", jsonrpsee::rpc_params![]),
    )
    .await??;
    from_hex_to_u64(&result)
}

/// For eth_syncing
pub async fn eth_syncing(client_rpc: &HttpClient) -> Result<Value> {
    let result: Value = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_syncing", jsonrpsee::rpc_params![]),
    )
    .await??;
    Ok(result)
}

/// For eth_gasPrice
pub async fn eth_gas_price(client_rpc: &HttpClient) -> Result<U256> {
    let result: String = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_gasPrice", jsonrpsee::rpc_params![]),
    )
    .await??;
    from_hex_to_u256(&result)
}

/// For eth_blockNumber
pub async fn eth_block_number(client_rpc: &HttpClient) -> Result<u64> {
    let result: String = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_blockNumber", jsonrpsee::rpc_params![]),
    )
    .await??;
    from_hex_to_u64(&result)
}

/// For eth_call
pub async fn eth_call(
    client_rpc: &HttpClient,
    call_params: Option<Value>,
    block_id: Option<BlockId>,
) -> Result<String> {
    let params = call_params.unwrap_or(serde_json::json!({}));
    let block_id = block_id.unwrap_or(BlockId::Latest).to_rpc_param();
    let result: String = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_call", jsonrpsee::rpc_params![params, block_id]),
    )
    .await??;

    Ok(result)
}

/// For eth_estimateGas
pub async fn estimate_gas(
    client_rpc: &HttpClient,
    tx_params: Option<Value>,
    block_id: Option<BlockId>,
) -> Result<u64> {
    let block_id = block_id.unwrap_or(BlockId::Latest).to_rpc_param();
    let tx_params = tx_params.ok_or(eyre::eyre!("tx_params is required"))?;
    let params = jsonrpsee::rpc_params![tx_params, block_id];
    let result: String =
        tokio::time::timeout(RPC_TIMEOUT, client_rpc.request("eth_estimateGas", params)).await??;
    from_hex_to_u64(&result)
}

/// For eth_getBalance
pub async fn get_balance(
    client_rpc: &HttpClient,
    address: &str,
    block_id: Option<BlockId>,
) -> Result<U256> {
    let block_id = block_id.unwrap_or(BlockId::Latest).to_rpc_param();
    let result: String = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_getBalance", jsonrpsee::rpc_params![address, block_id]),
    )
    .await??;
    from_hex_to_u256(&result)
}

/// For eth_getCode
pub async fn eth_get_code(
    client_rpc: &HttpClient,
    address: &str,
    block_id: Option<BlockId>,
) -> Result<String> {
    let block_id = block_id.unwrap_or(BlockId::Latest).to_rpc_param();
    let result: String = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_getCode", jsonrpsee::rpc_params![address, block_id]),
    )
    .await??;
    Ok(result)
}

/// For eth_getStorageAt
pub async fn eth_get_storage_at(
    client_rpc: &HttpClient,
    address: &str,
    position: &str,
    block_id: Option<BlockId>,
) -> Result<String> {
    let block_id = block_id.unwrap_or(BlockId::Latest).to_rpc_param();
    let result: String = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_getStorageAt", jsonrpsee::rpc_params![address, position, block_id]),
    )
    .await??;
    Ok(result)
}

/// For eth_getTransactionCount
pub async fn eth_get_transaction_count(
    client_rpc: &HttpClient,
    address: &str,
    block_id: Option<BlockId>,
) -> Result<u64> {
    let block_id = block_id.unwrap_or(BlockId::Latest).to_rpc_param();
    let result: String = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_getTransactionCount", jsonrpsee::rpc_params![address, block_id]),
    )
    .await??;
    from_hex_to_u64(&result)
}

/// For eth_getTransactionByHash
pub async fn eth_get_transaction_by_hash(client_rpc: &HttpClient, tx_hash: &str) -> Result<Value> {
    let result: Value = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_getTransactionByHash", jsonrpsee::rpc_params![tx_hash]),
    )
    .await??;
    Ok(result)
}

/// For eth_getTransactionReceipt
pub async fn eth_get_transaction_receipt(client_rpc: &HttpClient, tx_hash: &str) -> Result<Value> {
    let result: Value = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_getTransactionReceipt", jsonrpsee::rpc_params![tx_hash]),
    )
    .await??;
    Ok(result)
}

/// For eth_getTransactionByBlockHashAndIndex or eth_getTransactionByBlockNumberAndIndex
pub async fn eth_get_transaction_by_block_number_or_hash_and_index(
    client_rpc: &HttpClient,
    block_id: BlockId,
    index: &str,
) -> Result<Value> {
    // eth_getTransactionByBlockHashAndIndex
    if let BlockId::Hash(block_hash) = block_id {
        let result: Value = tokio::time::timeout(
            RPC_TIMEOUT,
            client_rpc.request(
                "eth_getTransactionByBlockHashAndIndex",
                jsonrpsee::rpc_params![block_hash, index],
            ),
        )
        .await??;
        return Ok(result);
    }

    // eth_getTransactionByBlockNumberAndIndex
    let block_id = block_id.to_rpc_param();
    let result: Value = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request(
            "eth_getTransactionByBlockNumberAndIndex",
            jsonrpsee::rpc_params![block_id, index],
        ),
    )
    .await??;
    Ok(result)
}

/// For eth_getBlockByNumber or eth_getBlockByHash
pub async fn eth_get_block_by_number_or_hash(
    client_rpc: &HttpClient,
    block_id: BlockId,
    fulltx: bool,
) -> Result<Value> {
    // eth_getBlockByHash
    if let BlockId::Hash(block_hash) = block_id {
        let result: Value = tokio::time::timeout(
            RPC_TIMEOUT,
            client_rpc.request("eth_getBlockByHash", jsonrpsee::rpc_params![block_hash, fulltx]),
        )
        .await??;
        return Ok(result);
    }

    // eth_getBlockByNumber
    let block_id = block_id.to_rpc_param();
    let result: Value = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_getBlockByNumber", jsonrpsee::rpc_params![block_id, fulltx]),
    )
    .await??;
    Ok(result)
}

/// For eth_getBlockReceipts
pub async fn eth_get_block_receipts(client_rpc: &HttpClient, block_id: BlockId) -> Result<Value> {
    let block_id = block_id.to_rpc_param();
    let result: Value = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_getBlockReceipts", jsonrpsee::rpc_params![block_id]),
    )
    .await??;
    Ok(result)
}

/// For eth_getBlockTransactionCountByHash
pub async fn eth_get_block_transaction_count_by_number_or_hash(
    client_rpc: &HttpClient,
    block_id: BlockId,
) -> Result<u64> {
    // eth_getBlockTransactionCountByHash
    if let BlockId::Hash(block_hash) = block_id {
        let result: String = tokio::time::timeout(
            RPC_TIMEOUT,
            client_rpc
                .request("eth_getBlockTransactionCountByHash", jsonrpsee::rpc_params![block_hash]),
        )
        .await??;
        return from_hex_to_u64(&result);
    }

    // eth_getBlockTransactionCountByNumber
    let block_id = block_id.to_rpc_param();
    let result: String = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc
            .request("eth_getBlockTransactionCountByNumber", jsonrpsee::rpc_params![block_id]),
    )
    .await??;
    from_hex_to_u64(&result)
}

/// For eth_getLogs
pub async fn eth_get_logs(
    client_rpc: &HttpClient,
    from_block: Option<BlockId>,
    to_block: Option<BlockId>,
    address: Option<&str>,
    topics: Option<Vec<String>>,
) -> Result<Value> {
    let mut filter = serde_json::Map::new();
    if let Some(from) = from_block {
        filter.insert("fromBlock".to_string(), json!(from.to_rpc_param()));
    }
    if let Some(to) = to_block {
        filter.insert("toBlock".to_string(), json!(to.to_rpc_param()));
    }
    if let Some(addr) = address {
        filter.insert("address".to_string(), json!(addr));
    }
    if let Some(t) = topics {
        filter.insert("topics".to_string(), json!(t));
    }
    let result: Value = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("eth_getLogs", jsonrpsee::rpc_params![Value::Object(filter)]),
    )
    .await??;
    Ok(result)
}

/// For txpool_content
pub async fn txpool_content(client_rpc: &HttpClient) -> Result<Value> {
    let result: Value = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("txpool_content", jsonrpsee::rpc_params![]),
    )
    .await??;
    Ok(result)
}

/// For txpool_status
pub async fn txpool_status(client_rpc: &HttpClient) -> Result<Value> {
    let result: Value = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("txpool_status", jsonrpsee::rpc_params![]),
    )
    .await??;
    Ok(result)
}
