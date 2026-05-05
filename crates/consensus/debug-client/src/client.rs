use alloy_primitives::B256;
use reth_node_api::{ConsensusEngineHandle, ExecutionPayload, PayloadTypes};
use reth_tracing::tracing::warn;
use ringbuffer::{AllocRingBuffer, RingBuffer};
use std::future::Future;
use tokio::sync::mpsc;

/// Supplies consensus client with new execution payloads sent in `tx` and a callback to find
/// specific payloads by number to fetch past finalized and safe block hashes.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait PayloadProvider: Send + Sync + 'static {
    /// The execution payload data type.
    type ExecutionData: ExecutionPayload;

    /// Runs a provider to send new execution payloads to the given sender.
    ///
    /// Note: This is expected to be spawned in a separate task, and as such it should ignore
    /// errors.
    fn subscribe_payloads(
        &self,
        tx: mpsc::Sender<Self::ExecutionData>,
    ) -> impl Future<Output = ()> + Send;

    /// Get a past execution payload by block number.
    fn get_payload(
        &self,
        block_number: u64,
    ) -> impl Future<Output = eyre::Result<Self::ExecutionData>> + Send;

    /// Get previous block hash using previous block hash buffer. If it isn't available (buffer
    /// started more recently than `offset`), fetch it using `get_payload`.
    fn get_or_fetch_previous_block(
        &self,
        previous_block_hashes: &AllocRingBuffer<B256>,
        current_block_number: u64,
        offset: usize,
    ) -> impl Future<Output = eyre::Result<B256>> + Send {
        async move {
            if let Some(hash) = get_hash_at_offset(previous_block_hashes, offset) {
                return Ok(hash);
            }

            // Return zero hash if the chain isn't long enough to have the block at the offset.
            let previous_block_number = match current_block_number.checked_sub(offset as u64) {
                Some(number) => number,
                None => return Ok(B256::default()),
            };
            let payload = self.get_payload(previous_block_number).await?;
            Ok(payload.block_hash())
        }
    }
}

/// Debug consensus client that sends FCUs and new payloads using recent payloads from an external
/// provider like Etherscan or an RPC endpoint.
#[derive(Debug)]
pub struct DebugConsensusClient<P: PayloadProvider, T: PayloadTypes> {
    /// Handle to execution client.
    engine_handle: ConsensusEngineHandle<T>,
    /// Provider to get consensus payloads from.
    block_provider: P,
}

impl<P: PayloadProvider, T: PayloadTypes> DebugConsensusClient<P, T> {
    /// Create a new debug consensus client with the given handle to execution
    /// client and block provider.
    pub const fn new(engine_handle: ConsensusEngineHandle<T>, block_provider: P) -> Self {
        Self { engine_handle, block_provider }
    }
}

impl<P, T> DebugConsensusClient<P, T>
where
    P: PayloadProvider<ExecutionData = T::ExecutionData> + Clone,
    T: PayloadTypes,
{
    /// Spawn the client to start sending FCUs and new payloads by periodically fetching recent
    /// payloads.
    pub async fn run(self) {
        let mut previous_block_hashes = AllocRingBuffer::new(65);
        let mut payload_stream = {
            let (tx, rx) = mpsc::channel::<P::ExecutionData>(64);
            let block_provider = self.block_provider.clone();
            tokio::spawn(async move {
                block_provider.subscribe_payloads(tx).await;
            });
            rx
        };

        while let Some(payload) = payload_stream.recv().await {
            let block_hash = payload.block_hash();
            let block_number = payload.block_number();

            previous_block_hashes.enqueue(block_hash);

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
            let _ = self.engine_handle.fork_choice_updated(state, None).await;
        }
    }
}

/// Looks up a block hash from the ring buffer at the given offset from the most recent entry.
///
/// Returns `None` if the buffer doesn't have enough entries to satisfy the offset.
fn get_hash_at_offset(buffer: &AllocRingBuffer<B256>, offset: usize) -> Option<B256> {
    buffer.len().checked_sub(offset + 1).and_then(|index| buffer.get(index).copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_hash_at_offset() {
        let mut buffer: AllocRingBuffer<B256> = AllocRingBuffer::new(65);

        // Empty buffer returns None for any offset
        assert_eq!(get_hash_at_offset(&buffer, 0), None);
        assert_eq!(get_hash_at_offset(&buffer, 1), None);

        // Push hashes 0..65
        for i in 0..65u8 {
            buffer.enqueue(B256::with_last_byte(i));
        }

        // offset=0 should return the most recent (64)
        assert_eq!(get_hash_at_offset(&buffer, 0), Some(B256::with_last_byte(64)));

        // offset=32 (safe block) should return hash 32
        assert_eq!(get_hash_at_offset(&buffer, 32), Some(B256::with_last_byte(32)));

        // offset=64 (finalized block) should return hash 0 (the oldest)
        assert_eq!(get_hash_at_offset(&buffer, 64), Some(B256::with_last_byte(0)));

        // offset=65 exceeds buffer, should return None
        assert_eq!(get_hash_at_offset(&buffer, 65), None);
    }

    #[test]
    fn test_get_hash_at_offset_insufficient_entries() {
        let mut buffer: AllocRingBuffer<B256> = AllocRingBuffer::new(65);

        // With only 1 entry, only offset=0 works
        buffer.enqueue(B256::with_last_byte(1));
        assert_eq!(get_hash_at_offset(&buffer, 0), Some(B256::with_last_byte(1)));
        assert_eq!(get_hash_at_offset(&buffer, 1), None);
        assert_eq!(get_hash_at_offset(&buffer, 32), None);
        assert_eq!(get_hash_at_offset(&buffer, 64), None);

        // With 33 entries, offset=32 works but offset=64 doesn't
        for i in 2..=33u8 {
            buffer.enqueue(B256::with_last_byte(i));
        }
        assert_eq!(get_hash_at_offset(&buffer, 32), Some(B256::with_last_byte(1)));
        assert_eq!(get_hash_at_offset(&buffer, 64), None);
    }
}
