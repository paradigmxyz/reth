use alloy_consensus::transaction::Transaction;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_txpool::TxpoolContent;
use serde::Serialize;
use std::error::Error;

#[derive(Debug, Serialize)]
struct TransactionInfo {
    from: String,
    to: Option<String>,
    value: String,
    gas: String,
    gas_price: Option<String>,
    max_fee_per_gas: Option<String>,
    max_priority_fee_per_gas: Option<String>,
    nonce: String,
    input: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    println!("连接到节点...");
    // 使用 ProviderBuilder 创建 provider
    let provider = ProviderBuilder::new().connect_http("http://localhost:8545".parse()?);

    println!("获取交易池内容...");
    // 获取交易池内容
    let txpool_content: TxpoolContent = provider.client().request("txpool_content", ()).await?;

    // 提取并打印待处理（pending）交易
    println!("\nPending Transactions:");
    for (address, txs) in txpool_content.pending {
        for (nonce, tx) in txs {
            let tx_info = TransactionInfo {
                from: format!("{:?}", address),
                to: tx.inner.to().map(|t| format!("{:?}", t)),
                value: format!("{:?}", tx.inner.value()),
                gas: format!("{:?}", tx.inner.gas_limit()),
                gas_price: tx.inner.gas_price().map(|p| format!("{:?}", p)),
                max_fee_per_gas: Some(format!("{:?}", tx.inner.max_fee_per_gas())),
                max_priority_fee_per_gas: Some(format!("{:?}", tx.inner.max_priority_fee_per_gas())),
                nonce: format!("{:?}", nonce),
                input: hex::encode(tx.inner.input()),
            };
            println!("{}", serde_json::to_string_pretty(&tx_info)?);
        }
    }

    // 提取并打印排队（queued）交易
    println!("\nQueued Transactions:");
    for (address, txs) in txpool_content.queued {
        for (nonce, tx) in txs {
            let tx_info = TransactionInfo {
                from: format!("{:?}", address),
                to: tx.inner.to().map(|t| format!("{:?}", t)),
                value: format!("{:?}", tx.inner.value()),
                gas: format!("{:?}", tx.inner.gas_limit()),
                gas_price: tx.inner.gas_price().map(|p| format!("{:?}", p)),
                max_fee_per_gas: Some(format!("{:?}", tx.inner.max_fee_per_gas())),
                max_priority_fee_per_gas: Some(format!("{:?}", tx.inner.max_priority_fee_per_gas())),
                nonce: format!("{:?}", nonce),
                input: hex::encode(tx.inner.input()),
            };
            println!("{}", serde_json::to_string_pretty(&tx_info)?);
        }
    }

    Ok(())
} 