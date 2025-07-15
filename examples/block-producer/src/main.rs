//! 区块生产示例：演示如何使用 engine_newPayload 和 engine_forkchoiceUpdated
//! 
//! 这个示例展示了完整的区块生产流程：
//! 1. 获取最新区块信息
//! 2. 构造新的执行负载 (ExecutionPayload)  
//! 3. 调用 engine_newPayload 提交新区块
//! 4. 调用 engine_forkchoiceUpdated 更新链头
//!
//! 运行前请确保：
//! - reth 节点运行在 localhost:8551
//! - 项目根目录有 jwt.hex 文件
//!
//! 运行命令：cargo run -p block-producer

use alloy_primitives::{Address, FixedBytes};
use alloy_rpc_types_engine::{ExecutionPayloadV3, ForkchoiceState, PayloadAttributes, PayloadStatus};
use eyre::Result;
use jsonwebtoken::{encode, Header, EncodingKey, Algorithm};
use rand::RngCore;
use reqwest::Client;
use serde_json::{json, Value};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(serde::Serialize)]
struct Claims {
    iat: u64,
    exp: u64,
}

fn create_jwt_token(secret: &str) -> Result<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let claims = Claims {
        iat: now,
        exp: now + 3600, // 1小时过期
    };
    
    let key = EncodingKey::from_secret(hex::decode(secret)?.as_ref());
    let token = encode(&Header::new(Algorithm::HS256), &claims, &key)?;
    Ok(token)
}

async fn make_rpc_call(
    client: &Client,
    jwt: &str,
    method: &str,
    params: Value,
) -> Result<Value> {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params
    });

    let response = client
        .post("http://127.0.0.1:8551")
        .bearer_auth(jwt)
        .json(&request)
        .send()
        .await?;

    let response_json: Value = response.json().await?;
    
    if let Some(error) = response_json.get("error") {
        return Err(eyre::eyre!("RPC error: {}", error));
    }
    
    Ok(response_json["result"].clone())
}

fn create_simple_payload(parent_block: &Value, block_number: u64) -> Result<ExecutionPayloadV3> {
    let mut rng = rand::thread_rng();
    let mut new_block_hash = [0u8; 32];
    rng.fill_bytes(&mut new_block_hash);
    
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let parent_hash = parent_block["hash"].as_str().unwrap_or("0x0");
    
    // 动态计算下一个区块的 base fee
    // 在开发网络中，当没有交易时，base fee 会按固定比例下降
    let parent_base_fee_str = parent_block["baseFeePerGas"].as_str().unwrap_or("0x3b9aca00");
    let parent_base_fee = u64::from_str_radix(parent_base_fee_str.trim_start_matches("0x"), 16)?;
    
    // 开发网络的 base fee 调整逻辑：当区块为空时，base fee 减少 1/8 (12.5%)
    let next_base_fee = parent_base_fee * 7 / 8; // 减少 12.5%
    let next_base_fee_hex = format!("0x{:x}", next_base_fee);
    
    println!("计算 base fee: {} -> {} (0x{:x})", parent_base_fee, next_base_fee, next_base_fee);
    
    let payload = json!({
        "parentHash": parent_hash,
        "feeRecipient": "0x0000000000000000000000000000000000000000",
        "stateRoot": "0xf09d8f7da5bc5036f8dd9536c953e2212390a46fb3e553ece2b7d419131537b1",
        "receiptsRoot": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
        "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        "prevRandao": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "blockNumber": format!("0x{:x}", block_number),
        "gasLimit": "0x1c9c380",
        "gasUsed": "0x0",
        "timestamp": format!("0x{:x}", timestamp),
        "extraData": "0x",
        "baseFeePerGas": next_base_fee_hex, // 使用计算出的 base fee
        "blockHash": format!("0x{}", hex::encode(new_block_hash)),
        "transactions": [],
        "withdrawals": [],
        "blobGasUsed": "0x0",
        "excessBlobGas": "0x0"
    });
    
    Ok(serde_json::from_value(payload)?)
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("🚀 简化区块生产示例");
    
    // 1. 读取 JWT
    let jwt_paths = ["jwt.hex", "./jwt.hex", "../jwt.hex", "../../jwt.hex"];
    let mut jwt_secret = String::new();
    
    for path in &jwt_paths {
        if Path::new(path).exists() {
            jwt_secret = std::fs::read_to_string(path)?.trim().to_string();
            println!("✅ 找到 JWT: {}", path);
            break;
        }
    }
    
    if jwt_secret.is_empty() {
        return Err(eyre::eyre!("未找到 jwt.hex 文件"));
    }
    
    let jwt = create_jwt_token(&jwt_secret)?;
    let client = Client::new();
    
    // 2. 获取最新区块
    println!("📊 获取最新区块...");
    let latest_block = make_rpc_call(&client, &jwt, "eth_getBlockByNumber", json!(["latest", true])).await?;
    
    let current_number = u64::from_str_radix(
        latest_block["number"].as_str().unwrap_or("0x0").trim_start_matches("0x"), 
        16
    )?;
    let parent_hash = latest_block["hash"].as_str().unwrap_or("0x0");
    
    println!("当前区块: #{}, 哈希: {}", current_number, parent_hash);
    
    // 3. 构造新载荷
    let new_block_number = current_number + 1;
    let payload = create_simple_payload(&latest_block, new_block_number)?;
    println!("🔨 构造区块 #{}", new_block_number);
    
    // 4. 提交载荷 (检测是否需要 requests hash)
    let is_prague = latest_block.get("requestsHash").is_some();
    let (method, params) = if is_prague {
        println!("📤 调用 engine_newPayloadV4 (Prague)...");
        let requests_hash = latest_block["requestsHash"].as_str().unwrap_or("0xe3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        ("engine_newPayloadV4", json!([payload, [], "0x0000000000000000000000000000000000000000000000000000000000000000", requests_hash]))
    } else {
        println!("📤 调用 engine_newPayloadV3 (Cancun)...");
        ("engine_newPayloadV3", json!([payload, [], "0x0000000000000000000000000000000000000000000000000000000000000000"]))
    };
    
    let new_payload_result = make_rpc_call(&client, &jwt, method, params).await?;
    println!("API 响应: {}", serde_json::to_string_pretty(&new_payload_result)?);
    
    let payload_status: PayloadStatus = serde_json::from_value(new_payload_result.clone())?;
    
    match payload_status.status.as_str() {
        "VALID" => println!("✅ 载荷有效！"),
        "INVALID" => {
            println!("❌ 载荷无效");
            return Err(eyre::eyre!("载荷验证失败"));
        },
        "SYNCING" => println!("⏳ 节点同步中，等待..."),
        "ACCEPTED" => println!("🔄 载荷已接受"),
        _ => println!("⚠️ 未知状态: {}", payload_status.status),
    }
    
    // 5. 更新分叉选择 - 使用我们刚刚验证的区块
    println!("🔄 更新分叉选择...");
    
    // 从 API 响应中获取新的有效哈希
    let new_head_hash = if let Some(hash) = new_payload_result.get("latestValidHash") {
        hash.as_str().unwrap_or("0x0000000000000000000000000000000000000000000000000000000000000000")
    } else {
        // 如果没有返回，使用我们构造的区块哈希
        &format!("0x{}", hex::encode(payload.payload_inner.payload_inner.block_hash.as_slice()))
    };
    
    println!("使用新的头部哈希: {}", new_head_hash);
    
    let forkchoice_state = ForkchoiceState {
        head_block_hash: new_head_hash.parse()?,
        safe_block_hash: payload.payload_inner.payload_inner.parent_hash,
        finalized_block_hash: payload.payload_inner.payload_inner.parent_hash,
    };
    
    // 第一步：仅更新 forkchoice，不要构建新区块
    let forkchoice_result = make_rpc_call(
        &client, 
        &jwt, 
        "engine_forkchoiceUpdatedV3", 
        json!([forkchoice_state, serde_json::Value::Null]) // 使用 null 代替 payload_attributes
    ).await?;
    
    if let Some(payload_id) = forkchoice_result.get("payloadId") {
        if !payload_id.is_null() {
            println!("✅ 获得 payloadId: {}", payload_id);
            
            // 6. 获取构建的载荷
            println!("📦 获取构建的载荷...");
            let get_payload_result = make_rpc_call(
                &client, 
                &jwt, 
                "engine_getPayloadV3", 
                json!([payload_id])
            ).await?;
            
            if let Some(execution_payload) = get_payload_result.get("executionPayload") {
                println!("✅ 成功构建区块 #{}", 
                    u64::from_str_radix(
                        execution_payload["blockNumber"].as_str().unwrap_or("0x0").trim_start_matches("0x"), 
                        16
                    ).unwrap_or(0)
                );
            }
        } else {
            println!("⚠️ 未获得 payloadId (节点可能仍在同步)");
        }
    }
    
    // 6. 验证区块是否真的被添加到链上
    println!("🔍 验证新区块是否被添加到链上...");
    tokio::time::sleep(std::time::Duration::from_secs(2)).await; // 等待一下
    
    let latest_block_after = make_rpc_call(&client, &jwt, "eth_getBlockByNumber", json!(["latest", false])).await?;
    let latest_number_after = u64::from_str_radix(
        latest_block_after["number"].as_str().unwrap_or("0x0").trim_start_matches("0x"), 
        16
    )?;
    
    if latest_number_after > current_number {
        println!("🎉 成功！新区块已添加到链上");
        println!("   区块号: {} -> {}", current_number, latest_number_after);
        println!("   新区块哈希: {}", latest_block_after["hash"].as_str().unwrap_or("unknown"));
    } else {
        println!("⚠️ 区块尚未被添加到链上 (可能需要更多时间或触发挖矿)");
    }
    
    println!("🎉 区块生产完成！");
    Ok(())
} 