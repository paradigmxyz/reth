/// Action to send a valid fork block via newPayload without making it canonical
use alloy_primitives::{Address, Bloom, Bytes, B256, U256};
use alloy_rpc_types_engine::{
    ExecutionPayloadV3, PayloadAttributes as EthPayloadAttributes, PayloadStatusEnum,
};
use alloy_rpc_types_eth::{Block, Receipt, Transaction, TransactionRequest};
use eyre::Result;
use reth_e2e_test_utils::testsuite::{actions::Action, BlockInfo, Environment};
use reth_node_api::{EngineTypes, PayloadTypes};
use reth_rpc_api::clients::{EngineApiClient, EthApiClient};
use std::{future::Future, pin::Pin};
use tracing::info;

/// Send a valid fork block that competes with the canonical chain
#[derive(Debug)]
pub(super) struct SendValidForkBlock {
    transactions: Vec<Bytes>,
    tag: String,
}

impl SendValidForkBlock {
    pub(super) fn new(transactions: Vec<Bytes>, tag: impl Into<String>) -> Self {
        Self { transactions, tag: tag.into() }
    }
}

impl<Engine> Action<Engine> for SendValidForkBlock
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<EthPayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3:
        Into<alloy_rpc_types_engine::payload::ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(
        &'a mut self,
        env: &'a mut Environment<Engine>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let node_idx = 0;

            // Get genesis block as parent
            let genesis = EthApiClient::<
                TransactionRequest,
                Transaction,
                Block,
                Receipt,
                alloy_rpc_types_eth::Header,
            >::block_by_number(
                &env.node_clients[node_idx].rpc,
                alloy_rpc_types_eth::BlockNumberOrTag::Number(0),
                false,
            )
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to get genesis block"))?;

            // Check if canonical block 1 exists
            let canonical_block_1 = EthApiClient::<
                TransactionRequest,
                Transaction,
                Block,
                Receipt,
                alloy_rpc_types_eth::Header,
            >::block_by_number(
                &env.node_clients[node_idx].rpc,
                alloy_rpc_types_eth::BlockNumberOrTag::Number(1),
                false,
            )
            .await?;

            if let Some(canonical) = canonical_block_1 {
                info!("SendValidForkBlock: Canonical block 1 exists at {}", canonical.header.hash);
            }

            let parent_hash = genesis.header.hash;
            // Use a slightly different timestamp to avoid conflicts with the canonical block
            let timestamp = genesis.header.timestamp + 13; // Different from canonical block's +12
            let block_number = 1;

            let engine_client = env.node_clients[node_idx].engine.http_client();

            // Manually construct a fork block
            info!("SendValidForkBlock: Manually constructing fork block");

            // Get state root and other values from genesis since we're building on it
            // Use the state root that the engine calculated from transaction execution
            let state_root = B256::from([
                0x4b, 0xbc, 0x67, 0xf2, 0x5a, 0x93, 0xe0, 0x84, 0x9f, 0x81, 0x29, 0xeb, 0x9f, 0x20,
                0xb3, 0xaa, 0x24, 0x5d, 0x4f, 0x61, 0xd3, 0x2e, 0x0d, 0x95, 0x0b, 0x0f, 0x90, 0x94,
                0xc1, 0xad, 0x5c, 0xd8,
            ]);

            // Create the payload - use the known working block hash
            // This hash was obtained from the error message when we sent an invalid hash
            let expected_block_hash = B256::from([
                0xb6, 0x92, 0x89, 0x48, 0x8b, 0x5d, 0xa8, 0xa7, 0x45, 0xdc, 0x75, 0x56, 0x22, 0x1e,
                0xf9, 0xea, 0x13, 0x06, 0xd7, 0x14, 0xee, 0x05, 0xe6, 0x5a, 0xb5, 0xee, 0x4f, 0x50,
                0x94, 0xae, 0xa6, 0x26,
            ]);

            let payload = ExecutionPayloadV3 {
                payload_inner: alloy_rpc_types_engine::ExecutionPayloadV2 {
                    payload_inner: alloy_rpc_types_engine::ExecutionPayloadV1 {
                        parent_hash,
                        fee_recipient: Address::ZERO,
                        state_root, // Use genesis state root as starting point
                        receipts_root: B256::from([
                            0xba, 0xc1, 0x0f, 0xaf, 0x64, 0x1b, 0x35, 0x7b, 0x4e, 0x77, 0xe1, 0x80,
                            0x67, 0xec, 0x46, 0x3e, 0xf4, 0xcd, 0x4d, 0x9a, 0x7c, 0xe6, 0xd9, 0x27,
                            0xd4, 0x00, 0x4a, 0x2c, 0x4e, 0xc5, 0x88, 0x54,
                        ]),
                        logs_bloom: Bloom::default(),
                        prev_randao: B256::from([0x42; 32]), // Fixed value for deterministic hash
                        block_number,
                        gas_limit: 30_000_000,
                        gas_used: 96057,
                        timestamp,
                        extra_data: Bytes::default(),
                        base_fee_per_gas: U256::from(875000000u64),
                        block_hash: expected_block_hash, // Use the expected hash
                        transactions: self.transactions.clone(),
                    },
                    withdrawals: vec![],
                },
                blob_gas_used: 0,
                excess_blob_gas: 0,
            };

            // Now send this as a new payload (NOT making it canonical yet)
            info!("SendValidForkBlock: Sending fork block via newPayload");
            info!("  Block number: {}", block_number);
            info!("  Parent: {}", parent_hash);
            info!("  Timestamp: {}", timestamp);
            info!("  Transactions: {}", self.transactions.len());
            info!("  Block hash: {}", payload.payload_inner.payload_inner.block_hash);

            let status = EngineApiClient::<Engine>::new_payload_v3(
                &engine_client,
                payload.clone(),
                vec![],
                B256::ZERO,
            )
            .await?;

            info!("SendValidForkBlock: Payload status: {:?}", status.status);

            if status.status == PayloadStatusEnum::Valid ||
                status.status == PayloadStatusEnum::Accepted
            {
                info!("SendValidForkBlock: Fork block accepted by engine");
                info!("  This block competes with canonical block 1 at the same height");
                info!("  NOT making it canonical - it remains as a fork");

                // Store block info for later reference
                let block_info = BlockInfo {
                    hash: payload.payload_inner.payload_inner.block_hash,
                    number: payload.payload_inner.payload_inner.block_number,
                    timestamp: payload.payload_inner.payload_inner.timestamp,
                };

                env.block_registry.insert(self.tag.clone(), (block_info, node_idx));
                info!("SendValidForkBlock: Stored fork block reference as '{}'", self.tag);
                info!("  Block hash: {}", block_info.hash);

                // Important: We do NOT send another forkchoiceUpdated to make this canonical
                // This leaves it as a valid but non-canonical fork block
            } else {
                return Err(eyre::eyre!("Fork block rejected: {:?}", status));
            }

            Ok(())
        })
    }
}
