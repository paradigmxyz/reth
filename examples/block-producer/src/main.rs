//! 区块生产示例：演示真实的 Engine API 工作流程
//! 
//! 这个示例展示了完整的区块生产流程：
//! 1. 获取最新区块信息
//! 2. 调用 engine_forkchoiceUpdated 请求构建新载荷
//! 3. 调用 engine_getPayload 获取构建的载荷
//! 4. 调用 engine_newPayload 提交载荷进行验证
//!
//! 这个流程模拟了共识客户端和执行客户端的交互方式
//!
//! 运行前请确保：
//! - reth 节点运行在 localhost:8551
//! - 项目根目录有 jwt.hex 文件
//!
//! 运行命令：cargo run -p block-producer

use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes, PayloadStatus};
use alloy_primitives::{Address, B256};
use eyre::Result;
use jsonwebtoken::{encode, Header as JwtHeader, EncodingKey, Algorithm};
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
    let token = encode(&JwtHeader::new(Algorithm::HS256), &claims, &key)?;
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

#[tokio::main]
async fn main() -> Result<()> {
    println!("🚀 真实的区块生产示例");
    println!("这个示例演示了共识客户端如何与执行客户端交互来生产区块\n");
    
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
    println!("📊 获取最新区块信息...");
    let latest_block = make_rpc_call(&client, &jwt, "eth_getBlockByNumber", json!(["latest", false])).await?;
    
    let current_number = u64::from_str_radix(
        latest_block["number"].as_str().unwrap_or("0x0").trim_start_matches("0x"), 
        16
    )?;
    let parent_hash = latest_block["hash"].as_str().unwrap_or("0x0");
    let parent_hash_b256: B256 = parent_hash.parse()?;
    
    println!("当前区块: #{}, 哈希: {}", current_number, parent_hash);
    
    // 3. 构造 ForkchoiceState - 告诉节点哪个是当前的头部区块
    let forkchoice_state = ForkchoiceState {
        head_block_hash: parent_hash_b256,
        safe_block_hash: parent_hash_b256,
        finalized_block_hash: parent_hash_b256,
    };
    
    // 4. 构造 PayloadAttributes - 告诉节点如何构建新区块
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let payload_attributes = PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO, // 在开发环境中使用零值
        suggested_fee_recipient: Address::ZERO, // 手续费接收地址
        withdrawals: Some(vec![]), // 空提款列表
        parent_beacon_block_root: Some(B256::ZERO), // Post-Cancun 需要提供这个字段
    };
    
    println!("🔧 构造载荷属性:");
    println!("  - 时间戳: {}", timestamp);
    println!("  - 建议的手续费接收者: 0x0000000000000000000000000000000000000000");
    
    // 5. 调用 engine_forkchoiceUpdated 请求构建载荷
    println!("\n📤 步骤 1: 调用 engine_forkchoiceUpdated 请求构建载荷...");
    
    let forkchoice_result = make_rpc_call(
        &client, 
        &jwt, 
        "engine_forkchoiceUpdatedV3", 
        json!([forkchoice_state, payload_attributes])
    ).await?;
    
    println!("✅ ForkchoiceUpdated 响应: {}", serde_json::to_string_pretty(&forkchoice_result)?);
    
    // 检查是否有 payloadId
    let payload_id = forkchoice_result.get("payloadId")
        .and_then(|id| id.as_str())
        .ok_or_else(|| eyre::eyre!("未收到 payloadId，无法继续"))?;
    
    println!("🎯 获得 payloadId: {}", payload_id);
    
    // 6. 调用 engine_getPayload 获取构建的载荷
    println!("\n📦 步骤 2: 调用 engine_getPayload 获取构建的载荷...");
    
    let get_payload_result = make_rpc_call(
        &client, 
        &jwt, 
        "engine_getPayloadV3", 
        json!([payload_id])
    ).await?;
    
    println!("✅ GetPayload 响应键: {:?}", get_payload_result.as_object().unwrap().keys().collect::<Vec<_>>());
    
    let execution_payload = get_payload_result.get("executionPayload")
        .ok_or_else(|| eyre::eyre!("响应中缺少 executionPayload"))?;
    
    let new_block_number = u64::from_str_radix(
        execution_payload["blockNumber"].as_str().unwrap_or("0x0").trim_start_matches("0x"), 
        16
    )?;
    
    println!("🎉 成功获取载荷！新区块号: #{}", new_block_number);
    println!("   区块哈希: {}", execution_payload.get("blockHash").and_then(|h| h.as_str()).unwrap_or("unknown"));
    println!("   Gas 使用量: {}", execution_payload.get("gasUsed").and_then(|g| g.as_str()).unwrap_or("0x0"));
    
    // 7. 调用 engine_newPayload 提交载荷进行验证
    println!("\n🔍 步骤 3: 调用 engine_newPayload 验证载荷...");
    
    // 检测是否需要 V4 (Prague)
    let is_prague = latest_block.get("requestsHash").is_some();
    let (method, params) = if is_prague {
        println!("检测到 Prague 硬分叉，使用 engine_newPayloadV4");
        let requests_hash = "0xe3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        ("engine_newPayloadV4", json!([execution_payload, [], "0x0000000000000000000000000000000000000000000000000000000000000000", requests_hash]))
    } else {
        println!("使用 engine_newPayloadV3");
        ("engine_newPayloadV3", json!([execution_payload, [], "0x0000000000000000000000000000000000000000000000000000000000000000"]))
    };
    
    let new_payload_result = make_rpc_call(&client, &jwt, method, params).await?;
    println!("✅ NewPayload 响应: {}", serde_json::to_string_pretty(&new_payload_result)?);
    
    let payload_status: PayloadStatus = serde_json::from_value(new_payload_result)?;
    
    match payload_status.status.as_str() {
        "VALID" => {
            println!("🎉 载荷验证成功！");
            println!("✨ 完整的区块生产流程演示完成！");
            println!("\n📋 总结:");
            println!("1. ✅ 通过 engine_forkchoiceUpdated 请求构建载荷");
            println!("2. ✅ 通过 engine_getPayload 获取构建的载荷"); 
            println!("3. ✅ 通过 engine_newPayload 验证载荷");
            println!("\n这就是真实环境中共识客户端和执行客户端的交互方式！");
        },
        "INVALID" => {
            println!("❌ 载荷无效");
            return Err(eyre::eyre!("载荷验证失败"));
        },
        "SYNCING" => println!("⏳ 节点同步中..."),
        "ACCEPTED" => println!("🔄 载荷已接受"),
        _ => println!("⚠️ 未知状态: {}", payload_status.status),
    }
    
    Ok(())
} 