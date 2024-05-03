/// TODO: Crate description

use alloy_consensus::TxEnvelope;
use alloy_eips::eip2718::Encodable2718;
use reth_node_api::EngineTypes;
use reth_node_core::{
    primitives::{B256},
    rpc::types::{BlockTransactions, ExecutionPayloadV2, ExecutionPayloadV3, RichBlock},
};
use reth_rpc_builder::auth::AuthServerHandle;
use reth_rpc_types::ExecutionPayloadV1;
use serde::Deserialize;
use std::time::Duration;
use tokio::time::sleep;

/// Fake consensus client that sends FCUs and new payloads using recent blocks from an external
/// provider, starting with Etherscan.
/// TODO: Naming - maybe "ExternalConsensusClient"? Or maybe simpler and just
///   "EtherscanConsensusClient"?
#[derive(Debug)]
pub struct RpcConsensusClient {
    /// HTTP client to fetch blocks
    http_client: reqwest::Client,
    /// Handle to execution client
    auth_server: AuthServerHandle,
    /// Etherscan API key
    etherscan_api_key: String,
    /// Etherscan base API URL
    etherscan_base_api_url: String,
}

impl RpcConsensusClient {
    /// Create a new fake consensus client that should sent FCUs and new payloads to `auth_server`.
    pub fn new(
        auth_server: AuthServerHandle,
        etherscan_api_key: String,
        etherscan_base_api_url: String,
    ) -> Self {
        Self {
            http_client: reqwest::Client::new(),
            auth_server,
            etherscan_api_key,
            etherscan_base_api_url,
        }
    }

    /// Spawn the client to start sending FCUs and new payloads by periodically fetching recent blocks.
    pub async fn spawn<T: EngineTypes>(&self) {
        // TODO: Add logs
        // TODO: Generalize over block-fetching code to support different sources

        let execution_client = self.auth_server.http_client();
        let mut last_block_number: Option<u64> = None;

        loop {
            let block: EtherscanBlockResponse = self
                .http_client
                .get(&self.etherscan_base_api_url)
                .query(&[
                    ("module", "proxy"),
                    ("action", "eth_getBlockByNumber"),
                    ("tag", "latest"),
                    ("boolean", "true"),
                    ("apikey", &self.etherscan_api_key),
                ])
                .send()
                .await
                .unwrap()
                .json()
                .await
                // TODO: Handle errors gracefully and do not stop the loop
                .unwrap();

            // Sleep if no new block is available
            if block.result.header.number == last_block_number {
                // TODO: Allow to configure this
                sleep(Duration::from_secs(3)).await;
                continue;
            }

            let payload = rich_block_to_execution_payload_v3(block.result);

            let block_hash = payload.block_hash();
            let block_number = payload.block_number();

            // Send new events to execution client
            reth_rpc_api::EngineApiClient::<T>::new_payload_v3(
                &execution_client,
                payload.execution_payload_v3,
                payload.versioned_hashes,
                payload.parent_beacon_block_root,
            )
            .await
            .unwrap();
            reth_rpc_api::EngineApiClient::<T>::fork_choice_updated_v3(
                &execution_client,
                reth_rpc_types::engine::ForkchoiceState {
                    head_block_hash: block_hash,
                    safe_block_hash: block_hash,
                    finalized_block_hash: block_hash,
                },
                None,
            )
            .await
            .unwrap();

            last_block_number = Some(block_number);

            // TODO: Allow to configure this
            sleep(Duration::from_secs(3)).await;
        }
    }
}

/// Context for a Cancun "new payload" with additional metadata.
#[derive(Debug)]
struct ExecutionNewPayload {
    execution_payload_v3: ExecutionPayloadV3,
    versioned_hashes: Vec<B256>,
    parent_beacon_block_root: B256,
}

impl ExecutionNewPayload {
    fn block_hash(&self) -> B256 {
        self.execution_payload_v3.payload_inner.payload_inner.block_hash
    }

    fn block_number(&self) -> u64 {
        self.execution_payload_v3.payload_inner.payload_inner.block_number
    }
}

/// Convert a rich block from RPC / Etherscan to params for an execution client's "new payload"
/// method. Assumes that the block contains full transactions.
fn rich_block_to_execution_payload_v3(block: RichBlock) -> ExecutionNewPayload {
    let transactions = match &block.transactions {
        BlockTransactions::Full(txs) => txs.clone(),
        // Empty array gets deserialized as BlockTransactions::Hashes.
        BlockTransactions::Hashes(txs) if txs.is_empty() => vec![],
        BlockTransactions::Hashes(_) | BlockTransactions::Uncle => {
            panic!("Received uncle block or hash-only transactions from Etherscan API")
        }
    };

    // Concatenate all blob hashes from all transactions in order
    // https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#specification
    let versioned_hashes = transactions
        .iter()
        .flat_map(|tx| tx.blob_versioned_hashes.clone().unwrap_or_default())
        .collect();

    // TODO: Do we want to handle errors more gracefully here or this is fine?
    let payload: ExecutionPayloadV3 = ExecutionPayloadV3 {
        payload_inner: ExecutionPayloadV2 {
            payload_inner: ExecutionPayloadV1 {
                parent_hash: block.header.parent_hash,
                fee_recipient: block.header.miner,
                state_root: block.header.state_root,
                receipts_root: block.header.receipts_root,
                logs_bloom: block.header.logs_bloom,
                prev_randao: block.header.mix_hash.unwrap(),
                block_number: block.header.number.unwrap(),
                gas_limit: block.header.gas_limit.try_into().unwrap(),
                gas_used: block.header.gas_used.try_into().unwrap(),
                timestamp: block.header.timestamp,
                extra_data: block.header.extra_data.clone(),
                base_fee_per_gas: block.header.base_fee_per_gas.unwrap().try_into().unwrap(),
                block_hash: block.header.hash.unwrap(),
                transactions: transactions
                    .into_iter()
                    .map(|tx| {
                        let envelope: TxEnvelope = tx.try_into().unwrap();
                        let mut buffer: Vec<u8> = vec![];
                        envelope.encode_2718(&mut buffer);
                        buffer.into()
                    })
                    .collect(),
            },
            withdrawals: block.withdrawals.clone().unwrap(),
        },
        blob_gas_used: block.header.blob_gas_used.unwrap().try_into().unwrap(),
        excess_blob_gas: block.header.excess_blob_gas.unwrap().try_into().unwrap(),
    };

    ExecutionNewPayload {
        execution_payload_v3: payload,
        versioned_hashes,
        parent_beacon_block_root: block.header.parent_beacon_block_root.unwrap(),
    }
}

#[derive(Deserialize, Debug)]
struct EtherscanBlockResponse {
    result: RichBlock,
}
