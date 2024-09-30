use alloy_consensus::TxEnvelope;
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::B256;
use alloy_rpc_types::{Block, BlockTransactions};
use alloy_rpc_types_engine::{ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3};
use reth_node_api::EngineTypes;
use reth_rpc_builder::auth::AuthServerHandle;
use reth_tracing::tracing::warn;
use ringbuffer::{AllocRingBuffer, RingBuffer};
use std::future::Future;
use tokio::sync::mpsc;

/// Supplies consensus client with new blocks sent in `tx` and a callback to find specific blocks
/// by number to fetch past finalized and safe blocks.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BlockProvider: Send + Sync + 'static {
    /// Runs a block provider to send new blocks to the given sender.
    ///
    /// Note: This is expected to be spawned in a separate task.
    fn subscribe_blocks(&self, tx: mpsc::Sender<Block>) -> impl Future<Output = ()> + Send;

    /// Get a past block by number.
    fn get_block(&self, block_number: u64) -> impl Future<Output = eyre::Result<Block>> + Send;

    /// Get previous block hash using previous block hash buffer. If it isn't available (buffer
    /// started more recently than `offset`), fetch it using `get_block`.
    fn get_or_fetch_previous_block(
        &self,
        previous_block_hashes: &AllocRingBuffer<B256>,
        current_block_number: u64,
        offset: usize,
    ) -> impl Future<Output = eyre::Result<B256>> + Send {
        async move {
            let stored_hash = previous_block_hashes
                .len()
                .checked_sub(offset)
                .and_then(|index| previous_block_hashes.get(index));
            if let Some(hash) = stored_hash {
                return Ok(*hash);
            }

            // Return zero hash if the chain isn't long enough to have the block at the offset.
            let previous_block_number = match current_block_number.checked_sub(offset as u64) {
                Some(number) => number,
                None => return Ok(B256::default()),
            };
            let block = self.get_block(previous_block_number).await?;
            Ok(block.header.hash)
        }
    }
}

/// Debug consensus client that sends FCUs and new payloads using recent blocks from an external
/// provider like Etherscan or an RPC endpoint.
#[derive(Debug)]
pub struct DebugConsensusClient<P: BlockProvider> {
    /// Handle to execution client.
    auth_server: AuthServerHandle,
    /// Provider to get consensus blocks from.
    block_provider: P,
}

impl<P: BlockProvider> DebugConsensusClient<P> {
    /// Create a new debug consensus client with the given handle to execution
    /// client and block provider.
    pub const fn new(auth_server: AuthServerHandle, block_provider: P) -> Self {
        Self { auth_server, block_provider }
    }
}

impl<P: BlockProvider + Clone> DebugConsensusClient<P> {
    /// Spawn the client to start sending FCUs and new payloads by periodically fetching recent
    /// blocks.
    pub async fn run<T: EngineTypes>(self) {
        let execution_client = self.auth_server.http_client();
        let mut previous_block_hashes = AllocRingBuffer::new(64);

        let mut block_stream = {
            let (tx, rx) = mpsc::channel::<Block>(64);
            let block_provider = self.block_provider.clone();
            tokio::spawn(async move {
                block_provider.subscribe_blocks(tx).await;
            });
            rx
        };

        while let Some(block) = block_stream.recv().await {
            let payload = block_to_execution_payload_v3(block);

            let block_hash = payload.block_hash();
            let block_number = payload.block_number();

            previous_block_hashes.push(block_hash);

            // Send new events to execution client
            let _ = reth_rpc_api::EngineApiClient::<T>::new_payload_v3(
                &execution_client,
                payload.execution_payload_v3,
                payload.versioned_hashes,
                payload.parent_beacon_block_root,
            )
            .await
                .inspect_err(|err|  {
                    warn!(target: "consensus::debug-client", %err, %block_hash,  %block_number, "failed to submit new payload to execution client");
                });

            // Load previous block hashes. We're using (head - 32) and (head - 64) as the safe and
            // finalized block hashes.
            let safe_block_hash = self.block_provider.get_or_fetch_previous_block(
                &previous_block_hashes,
                block_number,
                32,
            );
            let finalized_block_hash = self.block_provider.get_or_fetch_previous_block(
                &previous_block_hashes,
                block_number,
                64,
            );
            let (safe_block_hash, finalized_block_hash) =
                tokio::join!(safe_block_hash, finalized_block_hash);
            let (safe_block_hash, finalized_block_hash) = match (
                safe_block_hash,
                finalized_block_hash,
            ) {
                (Ok(safe_block_hash), Ok(finalized_block_hash)) => {
                    (safe_block_hash, finalized_block_hash)
                }
                (safe_block_hash, finalized_block_hash) => {
                    warn!(target: "consensus::debug-client", ?safe_block_hash, ?finalized_block_hash, "failed to fetch safe or finalized hash from etherscan");
                    continue;
                }
            };
            let state = alloy_rpc_types_engine::ForkchoiceState {
                head_block_hash: block_hash,
                safe_block_hash,
                finalized_block_hash,
            };
            let _ = reth_rpc_api::EngineApiClient::<T>::fork_choice_updated_v3(
                &execution_client,
                state,
                None,
            )
            .await
            .inspect_err(|err|  {
                warn!(target: "consensus::debug-client", %err, ?state, "failed to submit fork choice update to execution client");
            });
        }
    }
}

/// Cancun "new payload" event.
#[derive(Debug)]
pub struct ExecutionNewPayload {
    pub execution_payload_v3: ExecutionPayloadV3,
    pub versioned_hashes: Vec<B256>,
    pub parent_beacon_block_root: B256,
}

impl ExecutionNewPayload {
    /// Get block hash from block in the payload
    pub const fn block_hash(&self) -> B256 {
        self.execution_payload_v3.payload_inner.payload_inner.block_hash
    }

    /// Get block number from block in the payload
    pub const fn block_number(&self) -> u64 {
        self.execution_payload_v3.payload_inner.payload_inner.block_number
    }
}

/// Convert a block from RPC / Etherscan to params for an execution client's "new payload"
/// method. Assumes that the block contains full transactions.
pub fn block_to_execution_payload_v3(block: Block) -> ExecutionNewPayload {
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

    let payload: ExecutionPayloadV3 = ExecutionPayloadV3 {
        payload_inner: ExecutionPayloadV2 {
            payload_inner: ExecutionPayloadV1 {
                parent_hash: block.header.parent_hash,
                fee_recipient: block.header.miner,
                state_root: block.header.state_root,
                receipts_root: block.header.receipts_root,
                logs_bloom: block.header.logs_bloom,
                prev_randao: block.header.mix_hash.unwrap(),
                block_number: block.header.number,
                gas_limit: block.header.gas_limit,
                gas_used: block.header.gas_used,
                timestamp: block.header.timestamp,
                extra_data: block.header.extra_data.clone(),
                base_fee_per_gas: block.header.base_fee_per_gas.unwrap().try_into().unwrap(),
                block_hash: block.header.hash,
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
            withdrawals: block.withdrawals.clone().unwrap_or_default(),
        },
        blob_gas_used: block.header.blob_gas_used.unwrap(),
        excess_blob_gas: block.header.excess_blob_gas.unwrap(),
    };

    ExecutionNewPayload {
        execution_payload_v3: payload,
        versioned_hashes,
        parent_beacon_block_root: block.header.parent_beacon_block_root.unwrap(),
    }
}
