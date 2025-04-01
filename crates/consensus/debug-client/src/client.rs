use alloy_consensus::Sealable;
use alloy_primitives::B256;
use reth_node_api::{
    BeaconConsensusEngineHandle, BuiltPayload, EngineApiMessageVersion, ExecutionPayload,
    NodePrimitives, PayloadTypes,
};
use reth_primitives_traits::{Block, SealedBlock};
use reth_tracing::tracing::warn;
use ringbuffer::{AllocRingBuffer, RingBuffer};
use std::future::Future;
use tokio::sync::mpsc;

/// Supplies consensus client with new blocks sent in `tx` and a callback to find specific blocks
/// by number to fetch past finalized and safe blocks.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BlockProvider: Send + Sync + 'static {
    /// The block type.
    type Block: Block;

    /// Runs a block provider to send new blocks to the given sender.
    ///
    /// Note: This is expected to be spawned in a separate task, and as such it should ignore
    /// errors.
    fn subscribe_blocks(&self, tx: mpsc::Sender<Self::Block>) -> impl Future<Output = ()> + Send;

    /// Get a past block by number.
    fn get_block(
        &self,
        block_number: u64,
    ) -> impl Future<Output = eyre::Result<Self::Block>> + Send;

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
            Ok(block.header().hash_slow())
        }
    }
}

/// Debug consensus client that sends FCUs and new payloads using recent blocks from an external
/// provider like Etherscan or an RPC endpoint.
#[derive(Debug)]
pub struct DebugConsensusClient<P: BlockProvider, T: PayloadTypes> {
    /// Handle to execution client.
    engine_handle: BeaconConsensusEngineHandle<T>,
    /// Provider to get consensus blocks from.
    block_provider: P,
}

impl<P: BlockProvider, T: PayloadTypes> DebugConsensusClient<P, T> {
    /// Create a new debug consensus client with the given handle to execution
    /// client and block provider.
    pub const fn new(engine_handle: BeaconConsensusEngineHandle<T>, block_provider: P) -> Self {
        Self { engine_handle, block_provider }
    }
}

impl<P, T> DebugConsensusClient<P, T>
where
    P: BlockProvider + Clone,
    T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives: NodePrimitives<Block = P::Block>>>,
{
    /// Spawn the client to start sending FCUs and new payloads by periodically fetching recent
    /// blocks.
    pub async fn run(self) {
        let mut previous_block_hashes = AllocRingBuffer::new(64);

        let mut block_stream = {
            let (tx, rx) = mpsc::channel::<P::Block>(64);
            let block_provider = self.block_provider.clone();
            tokio::spawn(async move {
                block_provider.subscribe_blocks(tx).await;
            });
            rx
        };

        while let Some(block) = block_stream.recv().await {
            let payload = T::block_to_payload(SealedBlock::new_unhashed(block));

            let block_hash = payload.block_hash();
            let block_number = payload.block_number();

            previous_block_hashes.push(block_hash);

            // Send new events to execution client
            let _ = self.engine_handle.new_payload(payload).await;

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
            let _ = self
                .engine_handle
                .fork_choice_updated(state, None, EngineApiMessageVersion::V3)
                .await;
        }
    }
}
