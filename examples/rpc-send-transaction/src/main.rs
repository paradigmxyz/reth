//! 这个例子展示了如何在reth中通过RPC发送交易
//!
//! 运行方式:
//!
//! ```sh
//! cargo run -p rpc-send-transaction -- --rpc-url http://localhost:8545
//! ```

#![warn(unused_crate_dependencies)]

use alloy_consensus::{SignableTransaction, TxLegacy};
use alloy_eips::eip2718::Encodable2718;
use alloy_network::{EthereumWallet, TransactionBuilder, TxSigner};
use alloy_primitives::{Address, U256};
use alloy_provider::{Provider, ProviderBuilder, WalletProvider};
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use clap::Parser;
use eyre::Result;
use std::str::FromStr;
use tracing::{info, warn};

// 消除未使用 crate 的 warning
use alloy_genesis as _;
use alloy_rpc_client as _;
use alloy_signer as _;
use alloy_transport_http as _;
use futures_util as _;
use reqwest as _;
use reth_chainspec as _;
use reth_ethereum as _;
use reth_node_builder as _;
use reth_primitives as _;
use reth_provider as _;
use reth_rpc_eth_api as _;
use serde_json as _;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// RPC URL地址
    #[arg(long, default_value = "http://localhost:8545")]
    rpc_url: String,
    
    /// 接收者地址
    #[arg(long, default_value = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8")]
    to: String,
    
    /// 发送的ETH数量
    #[arg(long, default_value = "0.1")]
    amount: f64,
    
    /// 是否启用追踪日志
    #[arg(long)]
    trace: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    // 初始化日志
    if args.trace {
        tracing_subscriber::fmt::init();
    }
    
    println!("开始执行...");
    println!("连接到RPC: {}", args.rpc_url);
    
    // 1. 发送原始交易 (手动签名)
    println!("1. 尝试发送原始交易...");
    match send_raw_transaction(&args).await {
        Ok(_) => println!("原始交易发送完成"),
        Err(e) => println!("发送原始交易失败: {}", e),
    }
    
    // 2. 使用alloy Provider进行高级交易管理
    println!("2. 尝试使用Provider发送交易...");
    match send_with_alloy_provider(&args).await {
        Ok(_) => println!("Provider交易发送完成"),
        Err(e) => println!("Provider发送交易失败: {}", e),
    }
    
    println!("程序执行完成");
    Ok(())
}

/// 方法1: 发送原始交易 (eth_sendRawTransaction)
async fn send_raw_transaction(args: &Args) -> Result<()> {
    println!("开始准备原始交易...");
    
    // 创建provider
    let provider = ProviderBuilder::new().connect_http(args.rpc_url.parse()?);
    
    // 创建测试私钥 (开发网络中的测试账户)
    let private_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    let signer = PrivateKeySigner::from_str(private_key)?;
    let from = signer.address();
    println!("使用发送者地址: {}", from);
    
    println!("正在获取nonce...");
    // 获取当前nonce
    let nonce = match provider.get_transaction_count(from).await {
        Ok(n) => {
            println!("nonce: {:?}", n);
            n
        },
        Err(e) => {
            println!("获取nonce失败: {}", e);
            return Err(e.into());
        }
    };
    
    println!("正在获取gas价格...");
    // 获取gas价格
    let gas_price = match provider.get_gas_price().await {
        Ok(p) => {
            println!("gas_price: {:?}", p);
            p
        },
        Err(e) => {
            println!("获取gas价格失败: {}", e);
            return Err(e.into());
        }
    };
    
    println!("正在获取链ID...");
    // 获取链ID
    let chain_id = match provider.get_chain_id().await {
        Ok(id) => {
            println!("chain_id: {:?}", id);
            id
        },
        Err(e) => {
            println!("获取链ID失败: {}", e);
            return Err(e.into());
        }
    };
    
    // 创建传统交易
    let to_address = Address::from_str(&args.to)?;
    let value = U256::from((args.amount * 1e18) as u64);
    
    let mut tx = TxLegacy {
        chain_id: Some(chain_id),
        nonce: nonce.into(),
        gas_price: gas_price.into(),
        gas_limit: 21000,
        to: alloy_primitives::TxKind::Call(to_address),
        value,
        input: Default::default(),
    };
    
    println!("正在签名交易...");
    // 签名交易
    let signature = signer.sign_transaction(&mut tx).await?;
    let signed_tx = tx.into_signed(signature);
    
    // 编码为原始字节
    let raw_tx = signed_tx.encoded_2718();
    
    println!("正在发送交易...");
    // 发送交易
    match provider.send_raw_transaction(&raw_tx).await {
        Ok(pending_tx) => {
            let tx_hash = *pending_tx.tx_hash();
            println!("交易发送成功，tx_hash: {:?}", tx_hash);
        }
        Err(e) => {
            println!("交易发送失败: {}", e);
            return Err(e.into());
        }
    }
    
    Ok(())
}

/// 方法2: 使用alloy Provider的高级功能
async fn send_with_alloy_provider(args: &Args) -> Result<()> {
    println!("开始准备Provider交易...");
    
    // 创建私钥和钱包
    let private_key = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
    let signer = PrivateKeySigner::from_str(private_key)?;
    let wallet = EthereumWallet::from(signer);
    
    // 创建带钱包的Provider
    let provider_with_wallet = ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(args.rpc_url.parse()?);
    
    let from = provider_with_wallet.default_signer_address();
    let to_address = Address::from_str(&args.to)?;
    let value = U256::from((args.amount * 1e18) as u64);
    println!("使用发送者地址: {}", from);
    
    println!("正在获取区块信息...");
    // 获取当前基础费用
    let provider = ProviderBuilder::new().connect_http(args.rpc_url.parse()?);
    let block = match provider.get_block_by_number(alloy_rpc_types_eth::BlockNumberOrTag::Latest).await {
        Ok(b) => b,
        Err(e) => {
            println!("获取区块信息失败: {}", e);
            return Err(e.into());
        }
    };
    let base_fee = block.as_ref().and_then(|b| b.header.base_fee_per_gas).unwrap_or_default();
    
    // 创建交易请求
    let mut tx_request = TransactionRequest::default()
        .with_to(to_address)
        .with_value(value)
        .with_gas_limit(21000);
    
    // 如果支持EIP-1559，使用动态费用
    if base_fee > 0 {
        let max_priority_fee = 1_000_000_000u128; // 1 gwei tip
        let max_fee = (base_fee as u128) * 2 + max_priority_fee; // 2x base fee + 1 gwei tip
        
        tx_request = tx_request
            .with_max_fee_per_gas(max_fee)
            .with_max_priority_fee_per_gas(max_priority_fee);
    }
    
    println!("正在发送交易...");
    // 发送交易
    match provider_with_wallet.send_transaction(tx_request).await {
        Ok(pending_tx) => {
            let tx_hash = *pending_tx.tx_hash();
            println!("交易发送成功，tx_hash: {:?}", tx_hash);
        }
        Err(e) => {
            println!("交易发送失败: {}", e);
            return Err(e.into());
        }
    }
    
    Ok(())
} 