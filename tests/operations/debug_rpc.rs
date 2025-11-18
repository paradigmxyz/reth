use crate::operations::BlockId;
use eyre::Result;
use jsonrpsee::{core::client::ClientT, http_client::HttpClient};
use serde_json::{json, Value};
use std::time::Duration;

const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// For debug_traceBlockByHash or debug_traceBlockByNumber
pub async fn debug_trace_block(client_rpc: &HttpClient, block_id: BlockId) -> Result<Value> {
    // debug_traceBlockByHash. Leave tracer unset
    if let BlockId::Hash(block_hash) = block_id {
        let result: Value = tokio::time::timeout(
            RPC_TIMEOUT,
            client_rpc
                .request("debug_traceBlockByHash", jsonrpsee::rpc_params![block_hash, json!({})]),
        )
        .await??;
        return Ok(result);
    }

    // debug_traceBlockByNumber. Leave tracer unset
    let block_id = block_id.to_rpc_param();
    let result: Value = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("debug_traceBlockByNumber", jsonrpsee::rpc_params![block_id, json!({})]),
    )
    .await??;
    Ok(result)
}

/// For debug_traceTransaction
pub async fn debug_trace_transaction(client_rpc: &HttpClient, tx_hash: &str) -> Result<Value> {
    // Leave tracer unset
    let result: Value = tokio::time::timeout(
        RPC_TIMEOUT,
        client_rpc.request("debug_traceTransaction", jsonrpsee::rpc_params![tx_hash, json!({})]),
    )
    .await??;
    Ok(result)
}
