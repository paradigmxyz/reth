use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes};
use alloy_primitives::{Address, B256};
use std::time::{SystemTime, UNIX_EPOCH};
use rand::random;
use jsonwebtoken::{encode, Header, EncodingKey, Algorithm};

#[derive(serde::Serialize)]
struct Claims {
    iat: u64, // issued at
}

fn create_jwt_token(secret: &str) -> eyre::Result<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let claims = Claims { iat: now };
    
    // 将十六进制字符串转换为字节
    let secret_bytes = hex::decode(secret)?;
    
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(&secret_bytes),
    )?;
    
    Ok(token)
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // 读取 JWT 密钥
    let jwt_path = if std::path::Path::new("jwt.hex").exists() {
        "jwt.hex"
    } else if std::path::Path::new("../../jwt.hex").exists() {
        "../../jwt.hex"
    } else {
        return Err(eyre::eyre!("jwt.hex 文件未找到，请确保在项目根目录下存在 jwt.hex 文件"));
    };
    let jwt_secret = std::fs::read_to_string(jwt_path)?.trim().to_string();
    
    // 创建 JWT token
    let jwt_token = create_jwt_token(&jwt_secret)?;
    
    // 创建带有认证的 reqwest 客户端
    let client = reqwest::Client::new();
    
    // Engine API 端点
    let engine_url = "http://localhost:8551";
    
    println!("正在连接到 Engine API: {}", engine_url);

    // 首先使用 eth_getBlockByNumber 获取最新区块信息
    let get_block_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_getBlockByNumber",
        "params": ["latest", false]
    });

    let block_response = client
        .post(engine_url)
        .bearer_auth(&jwt_token)
        .json(&get_block_request)
        .send()
        .await?;

    if !block_response.status().is_success() {
        return Err(eyre::eyre!("获取区块失败: {}", block_response.status()));
    }

    let block_json: serde_json::Value = block_response.json().await?;
    
    println!("获取到最新区块: {:#}", block_json);

    // 从区块中提取必要信息
    let block_data = block_json["result"].as_object()
        .ok_or_else(|| eyre::eyre!("无法解析区块数据"))?;
    
    let head_block_hash = block_data["hash"].as_str()
        .ok_or_else(|| eyre::eyre!("无法获取区块哈希"))?;
    let parent_hash = block_data["parentHash"].as_str()
        .ok_or_else(|| eyre::eyre!("无法获取父区块哈希"))?;

    let head_hash = head_block_hash.parse::<B256>()?;
    let safe_hash = parent_hash.parse::<B256>()?;

    // 构造 ForkchoiceState
    let forkchoice_state = ForkchoiceState {
        head_block_hash: head_hash,
        safe_block_hash: safe_hash,
        finalized_block_hash: head_hash, // 简化示例，通常需要从已确认的区块获取
    };

    // 构造 PayloadAttributes
    let payload_attributes = PayloadAttributes {
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs(),
        prev_randao: B256::from(random::<[u8; 32]>()),
        suggested_fee_recipient: Address::ZERO, // 您可以设置为具体的接收地址
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };

    // 调用 engine_forkchoiceUpdatedV3
    let forkchoice_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "engine_forkchoiceUpdatedV3",
        "params": [
            serde_json::to_value(&forkchoice_state)?,
            serde_json::to_value(&payload_attributes)?
        ]
    });

    let forkchoice_response = client
        .post(engine_url)
        .bearer_auth(&jwt_token)
        .json(&forkchoice_request)
        .send()
        .await?;

    if !forkchoice_response.status().is_success() {
        return Err(eyre::eyre!("ForkchoiceUpdated 失败: {}", forkchoice_response.status()));
    }

    let response_json: serde_json::Value = forkchoice_response.json().await?;
    
    println!("ForkchoiceUpdated response: {:#}", response_json);

    // 尝试提取 payloadId（如果存在）
    let result = &response_json["result"];
    
    if let Some(payload_status) = result.get("payloadStatus") {
        println!("Payload status: {:#}", payload_status);
    }

    if let Some(payload_id) = result.get("payloadId") {
        println!("Generated payloadId: {}", payload_id);
        
        // 使用 payloadId 获取执行负载
        if let Some(payload_id_str) = payload_id.as_str() {
            let get_payload_request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "engine_getPayloadV3",
                "params": [payload_id_str]
            });
            
            let payload_response = client
                .post(engine_url)
                .bearer_auth(&jwt_token)
                .json(&get_payload_request)
                .send()
                .await?;
                
            if payload_response.status().is_success() {
                let payload_json: serde_json::Value = payload_response.json().await?;
                println!("Execution Payload: {:#}", payload_json);
            } else {
                println!("获取 payload 失败: {}", payload_response.status());
            }
        }
    }

    Ok(())
}
