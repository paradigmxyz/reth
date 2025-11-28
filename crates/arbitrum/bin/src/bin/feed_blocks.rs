//! Feed blocks from Arbitrum Sepolia to local arb-reth node via Engine API.
//!
//! This tool fetches blocks from the Arbitrum Sepolia RPC, encodes transactions properly
//! using Rust types, and submits them via the Engine API.

use alloy_eips::Encodable2718;
use alloy_primitives::{hex, Address, Bytes, B256, U256};
use alloy_rlp::Encodable;
use arb_alloy_consensus::tx::{
    ArbContractTx, ArbDepositTx, ArbInternalTx, ArbRetryTx, ArbSubmitRetryableTx, ArbTxType,
    ArbUnsignedTx,
};
use clap::Parser;
use eyre::Result;
use reth_arbitrum_primitives::{ArbTransactionSigned, ArbTypedTransaction};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Parser, Debug)]
#[command(name = "feed_blocks", about = "Feed blocks from Arbitrum Sepolia to local node")]
struct Args {
    /// Start block number
    #[arg(short, long, default_value = "1")]
    start: u64,

    /// End block number
    #[arg(short, long, default_value = "100")]
    end: u64,

    /// Local Engine API URL
    #[arg(long, default_value = "http://localhost:8551")]
    engine_url: String,

    /// Local RPC URL
    #[arg(long, default_value = "http://localhost:8547")]
    rpc_url: String,

    /// Arbitrum Sepolia RPC URL
    #[arg(long, default_value = "https://sepolia-rollup.arbitrum.io/rpc")]
    sepolia_url: String,

    /// JWT secret file path
    #[arg(long, default_value = "/home/dev/.local/share/reth/arbitrum-sepolia/jwt.hex")]
    jwt_file: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RpcResponse<T> {
    jsonrpc: String,
    id: u64,
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockResponse {
    hash: B256,
    parent_hash: B256,
    number: U256,
    timestamp: U256,
    gas_limit: U256,
    gas_used: U256,
    miner: Address,
    state_root: B256,
    receipts_root: B256,
    transactions_root: B256,
    logs_bloom: alloy_primitives::Bloom,
    #[serde(default)]
    mix_hash: B256,
    #[serde(default)]
    extra_data: Bytes,
    #[serde(default)]
    base_fee_per_gas: Option<U256>,
    #[serde(default)]
    parent_beacon_block_root: Option<B256>,
    #[serde(default)]
    nonce: alloy_primitives::B64,
    transactions: Vec<TxResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TxResponse {
    hash: B256,
    #[serde(rename = "type")]
    tx_type: U256,
    from: Address,
    #[serde(default)]
    to: Option<Address>,
    #[serde(default)]
    value: U256,
    #[serde(default)]
    input: Bytes,
    #[serde(default)]
    gas: U256,
    #[serde(default)]
    gas_price: Option<U256>,
    #[serde(default)]
    max_fee_per_gas: Option<U256>,
    #[serde(default)]
    max_priority_fee_per_gas: Option<U256>,
    #[serde(default)]
    nonce: U256,
    chain_id: Option<U256>,
    #[serde(default)]
    v: U256,
    #[serde(default)]
    r: U256,
    #[serde(default)]
    s: U256,
    #[serde(default)]
    access_list: Option<Vec<serde_json::Value>>,
    // Arbitrum-specific fields
    #[serde(default)]
    request_id: Option<B256>,
    #[serde(default)]
    l1_block_number: Option<U256>,
    #[serde(default)]
    deposit_value: Option<U256>,
    #[serde(default)]
    ticket_id: Option<B256>,
    #[serde(default)]
    refund_to: Option<Address>,
    #[serde(default)]
    max_refund: Option<U256>,
    #[serde(default)]
    submission_fee_refund: Option<U256>,
    #[serde(default)]
    l1_base_fee: Option<U256>,
    #[serde(default)]
    retry_to: Option<Address>,
    #[serde(default)]
    retry_value: Option<U256>,
    #[serde(default)]
    beneficiary: Option<Address>,
    #[serde(default)]
    max_submission_fee: Option<U256>,
    #[serde(default)]
    fee_refund_addr: Option<Address>,
    #[serde(default)]
    retry_data: Option<Bytes>,
}

fn convert_tx_to_signed(tx: &TxResponse) -> Result<ArbTransactionSigned> {
    let tx_type = tx.tx_type.to::<u8>();
    let chain_id = tx.chain_id.unwrap_or(U256::from(421614u64)); // Arbitrum Sepolia chain ID

    let transaction = if let Ok(arb_type) = ArbTxType::from_u8(tx_type) {
        match arb_type {
            ArbTxType::ArbitrumInternalTx => {
                // Type 0x6a - Internal transaction
                let internal = ArbInternalTx {
                    chain_id,
                    data: tx.input.clone(),
                };
                ArbTypedTransaction::Internal(internal)
            }
            ArbTxType::ArbitrumDepositTx => {
                // Type 0x64 - Deposit
                let deposit = ArbDepositTx {
                    chain_id,
                    l1_request_id: tx.request_id.unwrap_or_default(),
                    from: tx.from,
                    to: tx.to.unwrap_or_default(),
                    value: tx.value,
                };
                ArbTypedTransaction::Deposit(deposit)
            }
            ArbTxType::ArbitrumRetryTx => {
                // Type 0x68 - Retry
                let retry = ArbRetryTx {
                    chain_id,
                    nonce: tx.nonce.to::<u64>(),
                    from: tx.from,
                    gas_fee_cap: tx.max_fee_per_gas.or(tx.gas_price).unwrap_or_default(),
                    gas: tx.gas.to::<u64>(),
                    to: tx.to,
                    value: tx.value,
                    data: tx.input.clone(),
                    ticket_id: tx.ticket_id.unwrap_or_default(),
                    refund_to: tx.refund_to.unwrap_or(tx.from),
                    max_refund: tx.max_refund.unwrap_or_default(),
                    submission_fee_refund: tx.submission_fee_refund.unwrap_or_default(),
                };
                ArbTypedTransaction::Retry(retry)
            }
            ArbTxType::ArbitrumSubmitRetryableTx => {
                // Type 0x69 - Submit Retryable
                // RPC field `refundTo` maps to struct field `fee_refund_addr`
                let submit = ArbSubmitRetryableTx {
                    chain_id,
                    request_id: tx.request_id.unwrap_or_default(),
                    from: tx.from,
                    l1_base_fee: tx.l1_base_fee.unwrap_or_default(),
                    deposit_value: tx.deposit_value.unwrap_or_default(),
                    gas_fee_cap: tx.max_fee_per_gas.or(tx.gas_price).unwrap_or_default(),
                    gas: tx.gas.to::<u64>(),
                    retry_to: tx.retry_to,
                    retry_value: tx.retry_value.unwrap_or(tx.value),
                    beneficiary: tx.beneficiary.unwrap_or(tx.from),
                    max_submission_fee: tx.max_submission_fee.unwrap_or_default(),
                    fee_refund_addr: tx.refund_to.unwrap_or(tx.from),
                    retry_data: tx.retry_data.clone().unwrap_or_default(),
                };
                ArbTypedTransaction::SubmitRetryable(submit)
            }
            ArbTxType::ArbitrumUnsignedTx => {
                // Type 0x65 - Unsigned
                let unsigned = ArbUnsignedTx {
                    chain_id,
                    from: tx.from,
                    nonce: tx.nonce.to::<u64>(),
                    gas_fee_cap: tx.max_fee_per_gas.or(tx.gas_price).unwrap_or_default(),
                    gas: tx.gas.to::<u64>(),
                    to: tx.to,
                    value: tx.value,
                    data: tx.input.clone(),
                };
                ArbTypedTransaction::Unsigned(unsigned)
            }
            ArbTxType::ArbitrumContractTx => {
                // Type 0x66 - Contract
                let contract = ArbContractTx {
                    chain_id,
                    request_id: tx.request_id.unwrap_or_default(),
                    from: tx.from,
                    gas_fee_cap: tx.max_fee_per_gas.or(tx.gas_price).unwrap_or_default(),
                    gas: tx.gas.to::<u64>(),
                    to: tx.to,
                    value: tx.value,
                    data: tx.input.clone(),
                };
                ArbTypedTransaction::Contract(contract)
            }
            ArbTxType::ArbitrumLegacyTx => {
                eyre::bail!("Legacy Arbitrum tx type not supported")
            }
        }
    } else {
        // Standard EVM transaction types
        match tx_type {
            0 => {
                // Legacy
                let legacy = alloy_consensus::TxLegacy {
                    chain_id: tx.chain_id.map(|c| c.to::<u64>()),
                    nonce: tx.nonce.to::<u64>(),
                    gas_price: tx.gas_price.unwrap_or_default().to::<u128>(),
                    gas_limit: tx.gas.to::<u64>(),
                    to: if let Some(to) = tx.to {
                        alloy_primitives::TxKind::Call(to)
                    } else {
                        alloy_primitives::TxKind::Create
                    },
                    value: tx.value,
                    input: tx.input.clone(),
                };
                ArbTypedTransaction::Legacy(legacy)
            }
            1 => {
                // EIP-2930
                let eip2930 = alloy_consensus::TxEip2930 {
                    chain_id: chain_id.to::<u64>(),
                    nonce: tx.nonce.to::<u64>(),
                    gas_price: tx.gas_price.unwrap_or_default().to::<u128>(),
                    gas_limit: tx.gas.to::<u64>(),
                    to: if let Some(to) = tx.to {
                        alloy_primitives::TxKind::Call(to)
                    } else {
                        alloy_primitives::TxKind::Create
                    },
                    value: tx.value,
                    input: tx.input.clone(),
                    access_list: Default::default(),
                };
                ArbTypedTransaction::Eip2930(eip2930)
            }
            2 => {
                // EIP-1559
                let eip1559 = alloy_consensus::TxEip1559 {
                    chain_id: chain_id.to::<u64>(),
                    nonce: tx.nonce.to::<u64>(),
                    max_fee_per_gas: tx.max_fee_per_gas.unwrap_or_default().to::<u128>(),
                    max_priority_fee_per_gas: tx
                        .max_priority_fee_per_gas
                        .unwrap_or_default()
                        .to::<u128>(),
                    gas_limit: tx.gas.to::<u64>(),
                    to: if let Some(to) = tx.to {
                        alloy_primitives::TxKind::Call(to)
                    } else {
                        alloy_primitives::TxKind::Create
                    },
                    value: tx.value,
                    input: tx.input.clone(),
                    access_list: Default::default(),
                };
                ArbTypedTransaction::Eip1559(eip1559)
            }
            _ => eyre::bail!("Unsupported tx type: 0x{:02x}", tx_type),
        }
    };

    // Create signature
    // For legacy transactions (type 0), v is either:
    // - 27/28 for pre-EIP-155 (odd_y_parity = v - 27)
    // - chainId*2+35/36 for EIP-155 (odd_y_parity = (v - 35) % 2)
    // For EIP-2930/EIP-1559/etc (typed transactions), v is already 0 or 1
    let v_val = tx.v.to::<u64>();
    let odd_y_parity = if v_val == 0 || v_val == 1 {
        // Already normalized (EIP-2930, EIP-1559, etc.)
        v_val == 1
    } else if v_val == 27 || v_val == 28 {
        // Pre-EIP-155 legacy transaction
        v_val == 28
    } else {
        // EIP-155: v = chainId * 2 + 35 + recovery_id
        (v_val - 35) % 2 == 1
    };
    let signature = alloy_primitives::Signature::new(tx.r, tx.s, odd_y_parity);

    Ok(ArbTransactionSigned::new_unhashed(transaction, signature))
}

fn encode_tx_2718(tx: &ArbTransactionSigned, expected_hash: Option<B256>) -> Bytes {
    let mut buf = Vec::new();
    tx.encode_2718(&mut buf);

    // Debug: print encoded bytes and computed hash
    let computed_hash = alloy_primitives::keccak256(&buf);
    if let Some(exp) = expected_hash {
        if computed_hash != exp {
            println!("      WARNING: hash mismatch!");
            println!("        computed: {:?}", computed_hash);
            println!("        expected: {:?}", exp);
            println!("        encoded ({} bytes): 0x{}", buf.len(), hex::encode(&buf[..std::cmp::min(100, buf.len())]));
        } else {
            println!("      hash OK: {:?}", computed_hash);
        }
    }

    Bytes::from(buf)
}

async fn rpc_call<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<T> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1
    });

    let resp: RpcResponse<T> = client.post(url).json(&payload).send().await?.json().await?;

    if let Some(err) = resp.error {
        eyre::bail!("RPC error: {} - {}", err.code, err.message);
    }

    resp.result.ok_or_else(|| eyre::eyre!("No result in response"))
}

async fn engine_call<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: serde_json::Value,
    jwt_secret: &str,
) -> Result<T> {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};

    #[derive(Serialize)]
    struct Claims {
        iat: u64,
    }

    let iat = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let claims = Claims { iat };

    let key = hex::decode(jwt_secret)?;
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(&key),
    )?;

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1
    });

    let resp: RpcResponse<T> = client
        .post(url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&payload)
        .send()
        .await?
        .json()
        .await?;

    if let Some(err) = resp.error {
        eyre::bail!("Engine RPC error: {} - {}", err.code, err.message);
    }

    resp.result.ok_or_else(|| eyre::eyre!("No result in engine response"))
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PayloadStatus {
    status: String,
    latest_valid_hash: Option<B256>,
    validation_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForkchoiceUpdated {
    payload_status: PayloadStatus,
    payload_id: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    println!("Feed Blocks Tool");
    println!("================");
    println!("Start: {}, End: {}", args.start, args.end);
    println!("Engine URL: {}", args.engine_url);
    println!("Sepolia URL: {}", args.sepolia_url);

    // Read JWT secret
    let jwt_secret = std::fs::read_to_string(&args.jwt_file)?.trim().to_string();
    println!("JWT loaded from: {}", args.jwt_file);

    let client = reqwest::Client::new();

    // Get genesis block from local node
    let genesis: BlockResponse =
        rpc_call(&client, &args.rpc_url, "eth_getBlockByNumber", serde_json::json!(["0x0", true]))
            .await?;
    let genesis_hash = genesis.hash;
    println!("Genesis hash: {:?}", genesis_hash);

    for block_num in args.start..=args.end {
        println!("\n=== Block {} ===", block_num);

        // Fetch block from Sepolia
        let block: BlockResponse = rpc_call(
            &client,
            &args.sepolia_url,
            "eth_getBlockByNumber",
            serde_json::json!([format!("0x{:x}", block_num), true]),
        )
        .await?;

        println!("  Hash: {:?}", block.hash);
        println!("  StateRoot: {:?}", block.state_root);
        println!("  Transactions: {}", block.transactions.len());

        // Convert transactions
        let mut encoded_txs: Vec<Bytes> = Vec::new();
        for (i, tx) in block.transactions.iter().enumerate() {
            let tx_type = tx.tx_type.to::<u8>();
            println!("    tx[{}]: type=0x{:02x}, from={:?}", i, tx_type, tx.from);

            match convert_tx_to_signed(tx) {
                Ok(signed_tx) => {
                    let encoded = encode_tx_2718(&signed_tx, Some(tx.hash));
                    println!("      encoded: {} bytes", encoded.len());
                    encoded_txs.push(encoded);
                }
                Err(e) => {
                    println!("      ERROR encoding: {}", e);
                }
            }
        }

        // Build execution payload with Arbitrum-specific fields
        let payload = serde_json::json!({
            "parentHash": block.parent_hash,
            "feeRecipient": block.miner,
            "stateRoot": block.state_root,
            "receiptsRoot": block.receipts_root,
            "logsBloom": block.logs_bloom,
            "prevRandao": block.mix_hash,
            "blockNumber": format!("0x{:x}", block.number.to::<u64>()),
            "gasLimit": format!("0x{:x}", block.gas_limit.to::<u64>()),
            "gasUsed": format!("0x{:x}", block.gas_used.to::<u64>()),
            "timestamp": format!("0x{:x}", block.timestamp.to::<u64>()),
            "extraData": block.extra_data,
            "baseFeePerGas": block.base_fee_per_gas.map(|b| format!("0x{:x}", b.to::<u64>())).unwrap_or_else(|| "0x0".to_string()),
            "blockHash": block.hash,
            "transactions": encoded_txs,
            "withdrawals": [],
            "blobGasUsed": "0x0",
            "excessBlobGas": "0x0",
            // Arbitrum-specific: nonce carries transaction count info
            "nonce": block.nonce
        });

        println!("  Payload nonce: {:?}", block.nonce);

        let parent_beacon_root = block.parent_beacon_block_root.unwrap_or_default();

        // Submit payload
        let result: PayloadStatus = engine_call(
            &client,
            &args.engine_url,
            "engine_newPayloadV3",
            serde_json::json!([payload, [], parent_beacon_root]),
            &jwt_secret,
        )
        .await?;

        println!("  newPayload: status={}", result.status);
        if let Some(err) = &result.validation_error {
            println!("    validation_error: {}", err);
        }

        if result.status == "VALID" {
            // Update forkchoice
            let fc_state = serde_json::json!({
                "headBlockHash": block.hash,
                "safeBlockHash": block.hash,
                "finalizedBlockHash": genesis_hash
            });

            let fc_result: ForkchoiceUpdated = engine_call(
                &client,
                &args.engine_url,
                "engine_forkchoiceUpdatedV3",
                serde_json::json!([fc_state, null]),
                &jwt_secret,
            )
            .await?;

            println!("  forkchoice: status={}", fc_result.payload_status.status);
        }
    }

    // Check final state - use a simplified response struct since we only need basic fields
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SimpleBlockResponse {
        hash: B256,
        number: U256,
    }

    let latest: SimpleBlockResponse =
        rpc_call(&client, &args.rpc_url, "eth_getBlockByNumber", serde_json::json!(["latest", false]))
            .await?;
    println!(
        "\nLocal node now at block: {} ({:?})",
        latest.number.to::<u64>(),
        latest.hash
    );

    Ok(())
}
