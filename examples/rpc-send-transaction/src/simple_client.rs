//! 简单的RPC客户端示例
//! 
//! 运行方式:
//! ```sh
//! cargo run -p rpc-send-transaction --bin simple-client -- --rpc-url http://localhost:8545
//! ```

use rpc_send_transaction::RpcTxClient;
use alloy_primitives::{Address, U256};
use clap::Parser;
use eyre::Result;
use std::str::FromStr;
use tracing::info;

#[derive(Parser)]
#[command(name = "simple-client")]
#[command(about = "简单的RPC交易客户端")]
struct Args {
    /// RPC URL地址
    #[arg(long, default_value = "http://localhost:8545")]
    rpc_url: String,
    
    /// 私钥 (测试网络专用)
    #[arg(long, default_value = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80")]
    private_key: String,
    
    /// 接收者地址
    #[arg(long, default_value = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8")]
    to: String,
    
    /// 发送的ETH数量
    #[arg(long, default_value = "0.01")]
    amount: f64,
    
    /// 操作类型
    #[arg(long, default_value = "send")]
    action: String,
    
    /// 交易哈希 (用于查询)
    #[arg(long)]
    tx_hash: Option<String>,
    
    /// 地址 (用于查询余额)
    #[arg(long)]
    address: Option<String>,
    
    /// 确认数 (默认1)
    #[arg(long, default_value = "1")]
    confirmations: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    
    let args = Args::parse();
    
    // 创建RPC客户端
    let client = RpcTxClient::new(&args.rpc_url)?;
    
    // 显示网络信息
    client.get_network_info().await?;
    
    match args.action.as_str() {
        "send" => {
            let to = Address::from_str(&args.to)?;
            let value = U256::from((args.amount * 1e18) as u64);
            
            info!("=== 发送交易 ===");
            let tx_hash = client
                .send_transaction_with_wallet(&args.private_key, to, value, None)
                .await?;
                
            // 等待确认
            client.wait_for_confirmation(tx_hash, args.confirmations).await?;
        }
        
        "send-raw" => {
            let to = Address::from_str(&args.to)?;
            let value = U256::from((args.amount * 1e18) as u64);
            
            info!("=== 发送原始交易 ===");
            let tx_hash = client
                .send_raw_transaction(&args.private_key, to, value, None)
                .await?;
                
            client.wait_for_confirmation(tx_hash, args.confirmations).await?;
        }
        
        "balance" => {
            let address = if let Some(addr) = args.address {
                Address::from_str(&addr)?
            } else {
                use alloy_signer_local::PrivateKeySigner;
                let signer = PrivateKeySigner::from_str(&args.private_key)?;
                signer.address()
            };
            
            info!("=== 查询余额 ===");
            client.get_balance(address).await?;
        }
        
        "tx-info" => {
            if let Some(hash_str) = args.tx_hash {
                let tx_hash = alloy_primitives::TxHash::from_str(&hash_str)?;
                
                info!("=== 交易信息 ===");
                client.get_transaction_details(tx_hash).await?;
            } else {
                eprintln!("需要提供 --tx-hash 参数");
            }
        }
        
        "batch" => {
            info!("=== 批量发送交易 ===");
            
            // 创建批量交易 (发送给3个不同地址)
            let recipients = vec![
                "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
                "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC", 
                "0x90F79bf6EB2c4f870365E785982E1f101E93b906",
            ];
            
            let transactions: Result<Vec<_>> = recipients
                .iter()
                .map(|addr| {
                    let to = Address::from_str(addr)?;
                    let value = U256::from((args.amount * 1e18) as u64);
                    Ok((to, value))
                })
                .collect();
                
            let transactions = transactions?;
            
            let tx_hashes = client
                .send_batch_transactions(&args.private_key, transactions)
                .await?;
                
            info!("批量交易完成，发送了 {} 笔交易", tx_hashes.len());
            
            // 等待第一笔交易确认
            if let Some(first_tx) = tx_hashes.first() {
                client.wait_for_confirmation(*first_tx, args.confirmations).await?;
            }
        }
        
        _ => {
            eprintln!("未知操作: {}", args.action);
            eprintln!("支持的操作: send, send-raw, balance, tx-info, batch");
        }
    }
    
    Ok(())
} 