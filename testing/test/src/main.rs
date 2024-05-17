use alloy_provider::{Provider as AlloyProvider, ReqwestProvider};
use alloy_rpc_types::{transaction, Block, BlockId, EIP1186AccountProofResponse};
use alloy_transport_http::Http;
use reth_primitives::{Block as RethBlock, U256};
use url::Url;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let block_number = 18884920u64;
    let rpc_url = Url::parse("https://docs-demo.quiknode.pro/").expect("Invalid RPC URL");
    let provider: ReqwestProvider = ReqwestProvider::new_http(rpc_url);

    let alloy_block =
        provider.get_block_by_number(block_number.into(), true).await.unwrap().unwrap();
    if let Some(transactions) = alloy_block.transactions.as_transactions() {
        for (i, tx) in transactions.iter().enumerate() {
            if i == 3 {
                println!("Transaction: {:?}", tx.hash);
            }
        }
    }

    let block = RethBlock::try_from(alloy_block).unwrap();
    for (i, transaction) in block.body.iter().enumerate() {
        if i == 3 {
            println!("Transaction: {:?}", transaction);
        }
    }
    Ok(())
}
