use alloy_provider::{Provider as AlloyProvider, ReqwestProvider};
use alloy_rpc_types::{transaction, Block, BlockId, EIP1186AccountProofResponse, Parity};
use alloy_transport_http::Http;
use reth_primitives::{
    Block as RethBlock, Header as RethHeader, Transaction, TransactionSigned, U256,
};
use std::hash::Hash;
use url::Url;

mod alloy2reth;
use crate::alloy2reth::IntoReth;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let block_number = 18884920u64;
    let rpc_url = Url::parse("https://docs-demo.quiknode.pro/").expect("Invalid RPC URL");
    let provider: ReqwestProvider = ReqwestProvider::new_http(rpc_url);

    let mut alloy_block =
        provider.get_block_by_number(block_number.into(), true).await.unwrap().unwrap();
    alloy_block.transactions.as_transactions().unwrap()[3].signature.unwrap().y_parity =
        Some(Parity(true));
    if let Some(transactions) = alloy_block.transactions.as_transactions() {
        for (i, tx) in transactions.iter().enumerate() {
            if i == 3 || i == 4 || i == 5 {
                println!("Transaction: {:?}", tx.hash);
                println!("Signature: {:?}", tx.signature);
                println!("From: {:?}", tx.from);

                let reth_tx = Transaction::try_from(tx.clone()).unwrap();
                println!("Reth Transaction: {:?}", reth_tx);
                println!("hash {:?}", reth_tx.signature_hash());

                println!("====================");
            }
        }
    }

    // let withdrawals =
    //     alloy_block.withdrawals.unwrap().iter().map(|item| item.into_reth()).collect();
    let transactions: Vec<TransactionSigned> = alloy_block
        .transactions
        .as_transactions()
        .unwrap()
        .into_iter()
        .map(|item| item.clone().into_reth().with_hash())
        .collect();
    for (i, transaction) in transactions.iter().enumerate() {
        if i == 3 || i == 4 || i == 5 {
            println!("Transaction: {:?}", transaction);
        }
    }
    // let header: RethHeader = alloy_block.header.try_into().unwrap();

    // let reth_transactions = Transaction::try_from(alloy_block.transactions).unwrap();

    println!("===");

    let mut block = RethBlock::try_from(alloy_block).unwrap();
    for (i, transaction) in block.body.iter().enumerate() {
        if i == 3 || i == 4 || i == 5 {
            println!("Transaction: {:?}", transaction);
        }
    }
    Ok(())
}
