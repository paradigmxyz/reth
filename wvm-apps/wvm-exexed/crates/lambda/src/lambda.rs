use reqwest::Client;
use reth::{
    api::FullNodeComponents,
    primitives::{address, Address, TransactionSigned},
};
use reth_exex::ExExContext;
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;
use serde_json::{self, json};
use std::collections::HashMap;

pub const SEQ_ADDRESS: &str = "0x197f818c1313DC58b32D88078ecdfB40EA822614";
pub const LAMBDA_ENDPOINT: &str = "https://wvm-lambda-0755acbdae90.herokuapp.com";

fn is_transaction_to_sequencer(to: Address) -> bool {
    let addr_str = std::env::var("SEQUENCER_ADDRESS").unwrap_or(String::from(SEQ_ADDRESS));

    let addr = Address::parse_checksummed(addr_str, None).unwrap();

    to == addr
}

fn process_tx_sequencer(tx: &TransactionSigned) -> Option<String> {
    if let Some(to) = tx.transaction.to() {
        let is_tx_to_seq = is_transaction_to_sequencer(to);
        let is_input_empty = tx.transaction.input().is_empty();
        if is_tx_to_seq && !is_input_empty {
            return Some(tx.hash.to_string());
        }
    }

    None
}

pub async fn exex_lambda_processor<Node: FullNodeComponents>(
    mut ctx: ExExContext<Node>,
) -> eyre::Result<()> {
    let lambda_server = std::env::var("LAMBDA_ENDPOINT").unwrap_or(String::from(LAMBDA_ENDPOINT));

    let mut txs: Vec<String> = vec![];

    while let Some(notification) = ctx.notifications.recv().await {
        if let Some(committed_chain) = notification.committed_chain() {
            let client = reqwest::Client::new();
            let last_block = committed_chain.tip();

            for tx in last_block.body.iter() {
                let potential_hash = process_tx_sequencer(tx);
                if let Some(tx_hash) = potential_hash {
                    txs.push(tx_hash);
                }
            }

            let payload = json!({
                "bulk": true,
                "txs": txs
            });

            // TODO: Handle errors
            let _ = client
                .post(format!("{}/tx", lambda_server))
                .json::<serde_json::Value>(&payload)
                .send()
                .await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::lambda::is_transaction_to_sequencer;
    use reth::primitives::address;

    #[test]
    fn check_for_seq_address() {
        let to_addr = address!("197f818c1313DC58b32D88078ecdfB40EA822614");
        assert!(is_transaction_to_sequencer(to_addr));
    }
}
