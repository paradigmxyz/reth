use eyre::Result;
use jsonrpsee::{core::client::ClientT, http_client::HttpClient};
use serde_json::{json, Value};
use std::time::Duration;
use alloy_primitives::U256;

/// Returns the chain ID of the current network
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
///
/// # Returns
/// * `Result<u64>` - The chain ID as a u64, or an error
pub async fn eth_chain_id(client_rpc: &HttpClient) -> Result<u64> {
    // Set a timeout for the RPC call (10 seconds)
    let timeout = Duration::from_secs(10);
    
    // Make the RPC request to eth_chainId
    // eth_chainId returns a hex string, so we request it as String
    let result: String = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_chainId", jsonrpsee::rpc_params![])
    )
    .await??;
    
    // Convert hex string to u64
    // Remove "0x" prefix if present
    let hex_str = result.trim_start_matches("0x");
    
    // Parse hex string to u64
    let chain_id = u64::from_str_radix(hex_str, 16)?;
    
    Ok(chain_id)
}

/// Estimates gas for a transaction
pub async fn estimate_gas(
    client_rpc: &HttpClient,
    tx_params: Option<Value>,
) -> Result<u64> {
    let timeout = Duration::from_secs(10);
    
    let params = if let Some(tx) = tx_params {
        jsonrpsee::rpc_params![tx]
    } else {
        jsonrpsee::rpc_params![json!({})]
    };
    
    let result: String = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_estimateGas", params)
    )
    .await??;

    let gas = u64::from_str_radix(result.trim_start_matches("0x"), 16)?;
    Ok(gas)
}

/// Gets the balance of an address
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `address` - The address to get the balance for
///
/// # Returns
/// * `Result<U256>` - The balance in wei
pub async fn get_balance(client_rpc: &HttpClient, address: &str) -> Result<U256> {
    let timeout = Duration::from_secs(10);
    let result: String = tokio::time::timeout(timeout, client_rpc.request("eth_getBalance", jsonrpsee::rpc_params![address, "latest"]))
        .await??;
    let balance = U256::from_str_radix(result.trim_start_matches("0x"), 16)?;
    Ok(balance)
}

/// Gets the current block number
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
///
/// # Returns
/// * `Result<u64>` - The current block number
pub async fn eth_block_number(client_rpc: &HttpClient) -> Result<u64> {
    let timeout = Duration::from_secs(10);
    
    let result: String = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_blockNumber", jsonrpsee::rpc_params![])
    )
    .await??;
    
    let block_number = u64::from_str_radix(result.trim_start_matches("0x"), 16)?;
    Ok(block_number)
}

/// Gets a block by its number
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `block_number` - The block number
/// * `full_transactions` - Whether to return full transaction objects (true) or just hashes (false)
///
/// # Returns
/// * `Result<Value>` - The block data
pub async fn eth_get_block_by_number(
    client_rpc: &HttpClient,
    block_number: u64,
    full_transactions: bool,
) -> Result<Value> {
    let timeout = Duration::from_secs(10);
    let block_num_hex = format!("0x{:x}", block_number);
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getBlockByNumber", jsonrpsee::rpc_params![block_num_hex, full_transactions])
    )
    .await??;
    
    Ok(result)
}

/// Traces all transactions in a block by block hash
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `block_hash` - The block hash to trace
///
/// # Returns
/// * `Result<Value>` - The trace result containing traces for all transactions
pub async fn debug_trace_block_by_hash(
    client_rpc: &HttpClient,
    block_hash: &str,
) -> Result<Value> {
    let timeout = Duration::from_secs(30); // Tracing can take longer
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("debug_traceBlockByHash", jsonrpsee::rpc_params![block_hash, json!({})])
    )
    .await??;
    
    Ok(result)
}

/// Traces all transactions in a block by block number
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `block_number` - The block number to trace
///
/// # Returns
/// * `Result<Value>` - The trace result containing traces for all transactions
pub async fn debug_trace_block_by_number(
    client_rpc: &HttpClient,
    block_number: u64,
) -> Result<Value> {
    let timeout = Duration::from_secs(30); // Tracing can take longer
    let block_num_hex = format!("0x{:x}", block_number);
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("debug_traceBlockByNumber", jsonrpsee::rpc_params![block_num_hex, json!({})])
    )
    .await??;
    
    Ok(result)
}

/// Gets a block by its hash
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `block_hash` - The block hash
/// * `full_transactions` - Whether to return full transaction objects (true) or just hashes (false)
///
/// # Returns
/// * `Result<Value>` - The block data
pub async fn eth_get_block_by_hash(
    client_rpc: &HttpClient,
    block_hash: &str,
    full_transactions: bool,
) -> Result<Value> {
    let timeout = Duration::from_secs(10);
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getBlockByHash", jsonrpsee::rpc_params![block_hash, full_transactions])
    )
    .await??;
    
    Ok(result)
}

/// Traces a specific transaction by its hash
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `tx_hash` - The transaction hash to trace
///
/// # Returns
/// * `Result<Value>` - The trace result for the transaction
pub async fn debug_trace_transaction(
    client_rpc: &HttpClient,
    tx_hash: &str,
) -> Result<Value> {
    let timeout = Duration::from_secs(30); // Tracing can take longer
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("debug_traceTransaction", jsonrpsee::rpc_params![tx_hash, json!({})])
    )
    .await??;
    
    Ok(result)
}

/// Gets the sync status of the node
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
///
/// # Returns
/// * `Result<Value>` - False if not syncing, or an object with sync status if syncing
pub async fn eth_syncing(client_rpc: &HttpClient) -> Result<Value> {
    let timeout = Duration::from_secs(10);
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_syncing", jsonrpsee::rpc_params![])
    )
    .await??;
    
    Ok(result)
}

/// Gets the code at a given address
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `address` - The address to get code from
/// * `block` - The block parameter (e.g., "latest", "earliest", or a block number)
///
/// # Returns
/// * `Result<String>` - The code as a hex string
pub async fn eth_get_code(
    client_rpc: &HttpClient,
    address: &str,
    block: &str,
) -> Result<String> {
    let timeout = Duration::from_secs(10);
    
    let result: String = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getCode", jsonrpsee::rpc_params![address, block])
    )
    .await??;
    
    Ok(result)
}

/// Gets the transaction count (nonce) for an address
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `address` - The address to get the transaction count for
/// * `block` - The block parameter (e.g., "latest", "earliest", or a block number)
///
/// # Returns
/// * `Result<u64>` - The transaction count
pub async fn eth_get_transaction_count(
    client_rpc: &HttpClient,
    address: &str,
    block: &str,
) -> Result<u64> {
    let timeout = Duration::from_secs(10);
    
    let result: String = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getTransactionCount", jsonrpsee::rpc_params![address, block])
    )
    .await??;
    
    let count = u64::from_str_radix(result.trim_start_matches("0x"), 16)?;
    Ok(count)
}

/// Gets the current gas price
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
///
/// # Returns
/// * `Result<U256>` - The current gas price in wei
pub async fn eth_gas_price(client_rpc: &HttpClient) -> Result<U256> {
    let timeout = Duration::from_secs(10);
    
    let result: String = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_gasPrice", jsonrpsee::rpc_params![])
    )
    .await??;
    
    let gas_price = U256::from_str_radix(result.trim_start_matches("0x"), 16)?;
    Ok(gas_price)
}

/// Gets the value from a storage position at a given address
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `address` - The address to get storage from
/// * `position` - The storage position (as a hex string, e.g., "0x0")
/// * `block` - The block parameter (e.g., "latest", "earliest", or a block number)
///
/// # Returns
/// * `Result<String>` - The storage value as a hex string
pub async fn eth_get_storage_at(
    client_rpc: &HttpClient,
    address: &str,
    position: &str,
    block: &str,
) -> Result<String> {
    let timeout = Duration::from_secs(10);
    
    let result: String = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getStorageAt", jsonrpsee::rpc_params![address, position, block])
    )
    .await??;
    
    Ok(result)
}

/// Gets the number of transactions in a block by block hash
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `block_hash` - The block hash
///
/// # Returns
/// * `Result<u64>` - The number of transactions in the block
pub async fn eth_get_block_transaction_count_by_hash(
    client_rpc: &HttpClient,
    block_hash: &str,
) -> Result<u64> {
    let timeout = Duration::from_secs(10);
    
    let result: String = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getBlockTransactionCountByHash", jsonrpsee::rpc_params![block_hash])
    )
    .await??;
    
    let count = u64::from_str_radix(result.trim_start_matches("0x"), 16)?;
    Ok(count)
}

/// Gets the number of transactions in a block by block number
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `block_number` - The block number
///
/// # Returns
/// * `Result<u64>` - The number of transactions in the block
pub async fn eth_get_block_transaction_count_by_number(
    client_rpc: &HttpClient,
    block_number: u64,
) -> Result<u64> {
    let timeout = Duration::from_secs(10);
    let block_num_hex = format!("0x{:x}", block_number);
    
    let result: String = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getBlockTransactionCountByNumber", jsonrpsee::rpc_params![block_num_hex])
    )
    .await??;
    
    let count = u64::from_str_radix(result.trim_start_matches("0x"), 16)?;
    Ok(count)
}

/// Gets a transaction by block hash and transaction index
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `block_hash` - The block hash
/// * `index` - The transaction index (as a hex string, e.g., "0x0")
///
/// # Returns
/// * `Result<Value>` - The transaction data
pub async fn eth_get_transaction_by_block_hash_and_index(
    client_rpc: &HttpClient,
    block_hash: &str,
    index: &str,
) -> Result<Value> {
    let timeout = Duration::from_secs(10);
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getTransactionByBlockHashAndIndex", jsonrpsee::rpc_params![block_hash, index])
    )
    .await??;
    
    Ok(result)
}

/// Gets a transaction by block number and transaction index
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `block_number` - The block number
/// * `index` - The transaction index (as a hex string, e.g., "0x0")
///
/// # Returns
/// * `Result<Value>` - The transaction data
pub async fn eth_get_transaction_by_block_number_and_index(
    client_rpc: &HttpClient,
    block_number: u64,
    index: &str,
) -> Result<Value> {
    let timeout = Duration::from_secs(10);
    let block_num_hex = format!("0x{:x}", block_number);
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getTransactionByBlockNumberAndIndex", jsonrpsee::rpc_params![block_num_hex, index])
    )
    .await??;
    
    Ok(result)
}

/// Block identifier for RPC calls - can be either a number or hash
#[derive(Debug, Clone)]
pub enum BlockId {
    /// Block number as u64
    Number(u64),
    /// Block hash as string
    Hash(String),
}

impl BlockId {
    /// Convert BlockId to a string suitable for RPC calls
    fn to_rpc_param(&self) -> String {
        match self {
            BlockId::Number(num) => format!("0x{:x}", num),
            BlockId::Hash(hash) => hash.clone(),
        }
    }
}

/// Gets receipts for all transactions in a block by block number or hash
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `block_id` - The block identifier (number or hash)
///
/// # Returns
/// * `Result<Value>` - Array of receipt objects
pub async fn eth_get_block_receipts(
    client_rpc: &HttpClient,
    block_id: BlockId,
) -> Result<Value> {
    let timeout = Duration::from_secs(10);
    let block_param = block_id.to_rpc_param();
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getBlockReceipts", jsonrpsee::rpc_params![block_param])
    )
    .await??;
    
    Ok(result)
}

/// Gets the receipt of a transaction by transaction hash
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `tx_hash` - The transaction hash
///
/// # Returns
/// * `Result<Value>` - The transaction receipt
pub async fn eth_get_transaction_receipt(
    client_rpc: &HttpClient,
    tx_hash: &str,
) -> Result<Value> {
    let timeout = Duration::from_secs(10);
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getTransactionReceipt", jsonrpsee::rpc_params![tx_hash])
    )
    .await??;
    
    Ok(result)
}

/// Gets a transaction by its hash
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `tx_hash` - The transaction hash
///
/// # Returns
/// * `Result<Value>` - The transaction data
pub async fn eth_get_transaction_by_hash(
    client_rpc: &HttpClient,
    tx_hash: &str,
) -> Result<Value> {
    let timeout = Duration::from_secs(10);
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getTransactionByHash", jsonrpsee::rpc_params![tx_hash])
    )
    .await??;
    
    Ok(result)
}

/// Executes a new message call immediately without creating a transaction on the blockchain
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `call_params` - Optional call parameters (from, to, data, etc.)
///
/// # Returns
/// * `Result<String>` - The return value of the executed contract
pub async fn eth_call(
    client_rpc: &HttpClient,
    call_params: Option<Value>,
) -> Result<String> {
    let timeout = Duration::from_secs(10);
    
    let params = call_params.unwrap_or(serde_json::json!({}));
    let result: String = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_call", jsonrpsee::rpc_params![params, "latest"])
    )
    .await??;
    
    Ok(result)
}

/// Returns logs matching the given filter
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
/// * `from_block` - The starting block (e.g., "0x0", "latest", "earliest")
/// * `to_block` - The ending block (e.g., "0x0", "latest", "earliest")
/// * `address` - The contract address to filter logs by
///
/// # Returns
/// * `Result<Value>` - An array of log objects matching the filter
pub async fn eth_get_logs(
    client_rpc: &HttpClient,
    from_block: &str,
    to_block: &str,
    address: &str,
) -> Result<Value> {
    let timeout = Duration::from_secs(30);
    
    let filter = json!({
        "fromBlock": from_block,
        "toBlock": to_block,
        "address": address,
    });
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("eth_getLogs", jsonrpsee::rpc_params![filter])
    )
    .await??;
    
    Ok(result)
}

/// Returns the transactions that are in the transaction pool
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
///
/// # Returns
/// * `Result<Value>` - The transaction pool content
pub async fn txpool_content(client_rpc: &HttpClient) -> Result<Value> {
    let timeout = Duration::from_secs(30);
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("txpool_content", jsonrpsee::rpc_params![])
    )
    .await??;
    
    Ok(result)
}

/// Returns the number of transactions in the pool
///
/// # Arguments
/// * `client_rpc` - The JSON-RPC client to use for the call
///
/// # Returns
/// * `Result<Value>` - A map with pending and queued transaction counts
pub async fn txpool_status(client_rpc: &HttpClient) -> Result<Value> {
    let timeout = Duration::from_secs(10);
    
    let result: Value = tokio::time::timeout(
        timeout,
        client_rpc.request("txpool_status", jsonrpsee::rpc_params![])
    )
    .await??;
    
    Ok(result)
}
