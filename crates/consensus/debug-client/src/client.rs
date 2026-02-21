use alloy_consensus::{BlockHeader, Sealable};
use alloy_eips::{merge::EPOCH_SLOTS, BlockNumHash, BlockNumberOrTag};
use alloy_primitives::B256;
use alloy_rpc_types_engine::ForkchoiceState;
use reth_node_api::{
    BuiltPayload, ConsensusEngineHandle, EngineApiMessageVersion, NodePrimitives, PayloadTypes,
};
use reth_primitives_traits::{Block, SealedBlock};
use reth_tracing::tracing::warn;
use ringbuffer::{AllocRingBuffer, RingBuffer};
use std::{collections::HashMap, future::Future};
use tokio::sync::mpsc;

/// Debug consensus client that sends FCUs and new payloads using recent blocks from a
/// [`BlockProvider`].
#[derive(Debug)]
pub struct DebugConsensusClient<P: BlockProvider, T: PayloadTypes> {
    /// Handle to execution client.
    engine_handle: ConsensusEngineHandle<T>,
    /// Provider to get consensus blocks from.
    block_provider: P,
}

impl<P: BlockProvider, T: PayloadTypes> DebugConsensusClient<P, T> {
    /// Create a new debug consensus client with the given handle to execution
    /// client and block provider.
    pub const fn new(engine_handle: ConsensusEngineHandle<T>, block_provider: P) -> Self {
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
        let (tx, mut rx) = mpsc::channel(64);
        let block_provider = self.block_provider.clone();
        tokio::spawn(async move {
            block_provider.subscribe(tx).await;
        });

        while let Some(new_block) = rx.recv().await {
            let payload = T::block_to_payload(SealedBlock::new_unhashed(new_block.block));
            let _ = self.engine_handle.new_payload(payload).await;
            let _ = self
                .engine_handle
                .fork_choice_updated(new_block.forkchoice_state, None, EngineApiMessageVersion::V3)
                .await;
        }
    }
}

/// A new block event with its associated forkchoice state.
#[derive(Debug, Clone)]
pub struct NewBlock<B> {
    /// The new block.
    pub block: B,
    /// The forkchoice state to submit alongside this block.
    pub forkchoice_state: ForkchoiceState,
}

/// Supplies the consensus client with new blocks and their forkchoice state.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BlockProvider: Send + Sync + 'static {
    /// The block type.
    type Block: Block;

    /// Streams new blocks with their forkchoice state.
    fn subscribe(&self, tx: mpsc::Sender<NewBlock<Self::Block>>)
        -> impl Future<Output = ()> + Send;
}

/// Provides raw blocks without forkchoice information.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait RawBlockProvider: Send + Sync + 'static {
    /// The block type.
    type Block: Block;

    /// Streams new blocks.
    fn subscribe_blocks(&self, tx: mpsc::Sender<Self::Block>) -> impl Future<Output = ()> + Send;

    /// Gets a block by number or tag (e.g. `Safe`, `Finalized`, `Number(u64)`).
    fn get_block(
        &self,
        block: BlockNumberOrTag,
    ) -> impl Future<Output = eyre::Result<Self::Block>> + Send;
}

/// Determines how the [`ForkchoiceProvider`] derives safe and finalized block hashes.
#[derive(Debug, Clone, Copy)]
pub enum ForkchoiceMode {
    /// Derives safe and finalized hashes from fixed offsets relative to the head.
    ///
    /// Uses a ring buffer to look up past block hashes, falling back to
    /// [`RawBlockProvider::get_block`] for blocks outside the buffer.
    Offset {
        /// Offset from head for the safe block hash.
        safe: usize,
        /// Offset from head for the finalized block hash.
        finalized: usize,
    },
    /// Buffers incoming blocks and only yields them once they are finalized.
    ///
    /// On each new block, the current finalized block number is fetched via
    /// [`BlockNumberOrTag::Finalized`]. All buffered blocks up to that number are then
    /// emitted with head=safe=finalized set to the block's own hash.
    ///
    /// **Note:** Requires a provider that supports [`BlockNumberOrTag::Finalized`] lookups
    /// (e.g., [`crate::RpcBlockProvider`]). Not supported by [`crate::EtherscanBlockProvider`].
    Finalized,
    /// Fetches the safe and finalized blocks by their respective tags
    /// ([`BlockNumberOrTag::Safe`] and [`BlockNumberOrTag::Finalized`]) from the provider.
    ///
    /// **Note:** Requires a provider that supports tag-based lookups
    /// (e.g., [`crate::RpcBlockProvider`]). Not supported by [`crate::EtherscanBlockProvider`].
    Tag,
}

impl ForkchoiceMode {
    /// The default offset mode: safe = head - 32, finalized = head - 64.
    pub const DEFAULT_OFFSET: Self =
        Self::Offset { safe: EPOCH_SLOTS as usize, finalized: (EPOCH_SLOTS * 2) as usize };
}

impl Default for ForkchoiceMode {
    fn default() -> Self {
        Self::DEFAULT_OFFSET
    }
}

/// Wraps a [`RawBlockProvider`] and derives forkchoice state for each block
/// according to the configured [`ForkchoiceMode`].
#[derive(Debug, Clone)]
pub struct ForkchoiceProvider<P> {
    /// Inner raw block provider.
    inner: P,
    /// How to derive safe and finalized hashes.
    mode: ForkchoiceMode,
}

impl<P> ForkchoiceProvider<P> {
    /// Creates a new provider with the default offset mode (safe=32, finalized=64).
    pub fn new(inner: P) -> Self {
        Self { inner, mode: ForkchoiceMode::default() }
    }

    /// Creates a new provider with the given mode.
    pub const fn with_mode(inner: P, mode: ForkchoiceMode) -> Self {
        Self { inner, mode }
    }
}

impl<P> BlockProvider for ForkchoiceProvider<P>
where
    P: RawBlockProvider + Clone,
{
    type Block = P::Block;

    async fn subscribe(&self, tx: mpsc::Sender<NewBlock<Self::Block>>) {
        let (block_tx, mut block_rx) = mpsc::channel::<P::Block>(64);
        let inner = self.inner.clone();
        tokio::spawn(async move {
            inner.subscribe_blocks(block_tx).await;
        });

        match self.mode {
            ForkchoiceMode::Offset { safe: safe_offset, finalized: finalized_offset } => {
                let mut previous_block_hashes =
                    AllocRingBuffer::new(safe_offset.max(finalized_offset) + 1);

                while let Some(block) = block_rx.recv().await {
                    let block_hash = block.header().hash_slow();
                    let block_number = block.header().number();

                    previous_block_hashes.enqueue(BlockNumHash::new(block_number, block_hash));

                    let safe_block_hash = get_or_fetch_hash(
                        &self.inner,
                        &previous_block_hashes,
                        block_number,
                        safe_offset,
                    );
                    let finalized_block_hash = get_or_fetch_hash(
                        &self.inner,
                        &previous_block_hashes,
                        block_number,
                        finalized_offset,
                    );

                    let (safe_block_hash, finalized_block_hash) =
                        tokio::join!(safe_block_hash, finalized_block_hash);

                    let (safe_block_hash, finalized_block_hash) = match (
                        safe_block_hash,
                        finalized_block_hash,
                    ) {
                        (Ok(safe), Ok(finalized)) => (safe, finalized),
                        (safe_block_hash, finalized_block_hash) => {
                            warn!(target: "consensus::debug-client", ?safe_block_hash, ?finalized_block_hash, "failed to fetch safe or finalized hash");
                            continue;
                        }
                    };

                    let new_block = NewBlock {
                        block,
                        forkchoice_state: ForkchoiceState {
                            head_block_hash: block_hash,
                            safe_block_hash,
                            finalized_block_hash,
                        },
                    };

                    if tx.send(new_block).await.is_err() {
                        break;
                    }
                }
            }
            ForkchoiceMode::Finalized => {
                let mut buffer = FinalizedBlockBuffer::new();

                while let Some(block) = block_rx.recv().await {
                    buffer.insert(block);

                    let finalized = match self.inner.get_block(BlockNumberOrTag::Finalized).await {
                        Ok(b) => BlockNumHash::new(b.header().number(), b.header().hash_slow()),
                        Err(err) => {
                            warn!(target: "consensus::debug-client", %err, "failed to fetch finalized block");
                            continue;
                        }
                    };

                    let finalized_blocks = buffer.drain_finalized(finalized);

                    for finalized_block in finalized_blocks {
                        let hash = finalized_block.header().hash_slow();
                        let new_block = NewBlock {
                            block: finalized_block,
                            forkchoice_state: ForkchoiceState {
                                head_block_hash: hash,
                                safe_block_hash: hash,
                                finalized_block_hash: hash,
                            },
                        };

                        if tx.send(new_block).await.is_err() {
                            return;
                        }
                    }
                }
            }
            ForkchoiceMode::Tag => {
                while let Some(block) = block_rx.recv().await {
                    let block_hash = block.header().hash_slow();

                    let safe = self.inner.get_block(BlockNumberOrTag::Safe);
                    let finalized = self.inner.get_block(BlockNumberOrTag::Finalized);
                    let (safe, finalized) = tokio::join!(safe, finalized);

                    let (safe_block_hash, finalized_block_hash) = match (safe, finalized) {
                        (Ok(safe), Ok(finalized)) => {
                            (safe.header().hash_slow(), finalized.header().hash_slow())
                        }
                        (safe, finalized) => {
                            warn!(target: "consensus::debug-client", ?safe, ?finalized, "failed to fetch safe or finalized block by tag");
                            continue;
                        }
                    };

                    let new_block = NewBlock {
                        block,
                        forkchoice_state: ForkchoiceState {
                            head_block_hash: block_hash,
                            safe_block_hash,
                            finalized_block_hash,
                        },
                    };

                    if tx.send(new_block).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

/// Fetches a block hash at the given offset, first checking the ring buffer, then falling back
/// to the raw block provider.
async fn get_or_fetch_hash<P: RawBlockProvider>(
    provider: &P,
    buffer: &AllocRingBuffer<BlockNumHash>,
    current_block_number: u64,
    offset: usize,
) -> eyre::Result<B256> {
    let target_number = match current_block_number.checked_sub(offset as u64) {
        Some(number) => number,
        None => return Ok(B256::default()),
    };

    if let Some(hash) = find_hash_by_number(buffer, target_number) {
        return Ok(hash);
    }

    let block = provider.get_block(BlockNumberOrTag::Number(target_number)).await?;
    Ok(block.header().hash_slow())
}

/// Finds a block hash by block number in the ring buffer, searching from newest to oldest.
fn find_hash_by_number(buffer: &AllocRingBuffer<BlockNumHash>, target_number: u64) -> Option<B256> {
    buffer.iter().rev().find(|entry| entry.number == target_number).map(|entry| entry.hash)
}

/// Buffers blocks and releases them once they become finalized.
///
/// Blocks are indexed by their hash. When [`drain_finalized`](Self::drain_finalized) is called,
/// it walks the `parent_hash` chain from the finalized block backwards, collecting all buffered
/// blocks that belong to the finalized chain. Any remaining blocks at or below the finalized
/// height are pruned as sidechain blocks.
#[derive(Debug)]
pub struct FinalizedBlockBuffer<B> {
    blocks: HashMap<B256, B>,
    /// The last finalized block's number and hash.
    last_finalized: Option<BlockNumHash>,
}

impl<B: Block> FinalizedBlockBuffer<B> {
    /// Creates an empty buffer.
    pub fn new() -> Self {
        Self { blocks: HashMap::new(), last_finalized: None }
    }

    /// Inserts a block into the buffer.
    pub fn insert(&mut self, block: B) {
        let hash = block.header().hash_slow();
        self.blocks.insert(hash, block);
    }

    /// Walks the parent hash chain from `finalized` back to the last known finalized block,
    /// removes all matching blocks from the buffer, and returns them in ascending order
    /// (oldest first).
    ///
    /// Any remaining buffered blocks at or below the finalized height are pruned as sidechain
    /// blocks that can no longer become canonical.
    ///
    /// Returns an empty vec if the finalized hash is not in the buffer.
    pub fn drain_finalized(&mut self, finalized: BlockNumHash) -> Vec<B> {
        if !self.blocks.contains_key(&finalized.hash) {
            return Vec::new();
        }

        // Walk backwards from the finalized hash, collecting hashes of blocks to drain.
        let mut chain = Vec::new();
        let mut current = finalized.hash;

        while self.blocks.contains_key(&current) {
            chain.push(current);

            // Stop if we've reached the previous finalized boundary.
            if self.last_finalized.as_ref().is_some_and(|last| last.hash == current) {
                break;
            }

            let parent = self.blocks[&current].header().parent_hash();
            current = parent;
        }

        self.last_finalized = Some(finalized);

        // Reverse so we return oldest-first, then remove from map.
        chain.reverse();
        let result: Vec<B> =
            chain.into_iter().filter_map(|hash| self.blocks.remove(&hash)).collect();

        // Prune any sidechain blocks at or below the finalized height.
        self.blocks.retain(|_, b| b.header().number() > finalized.number);

        result
    }

    /// Returns the number of buffered blocks.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Returns `true` if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

impl<B: Block> Default for FinalizedBlockBuffer<B> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_hash_by_number() {
        let mut buffer: AllocRingBuffer<BlockNumHash> = AllocRingBuffer::new(65);

        // Empty buffer returns None for any number
        assert_eq!(find_hash_by_number(&buffer, 0), None);
        assert_eq!(find_hash_by_number(&buffer, 1), None);

        // Push entries for block numbers 0..65
        for i in 0..65u64 {
            buffer.enqueue(BlockNumHash::new(i, B256::with_last_byte(i as u8)));
        }

        // Look up the most recent (block 64)
        assert_eq!(find_hash_by_number(&buffer, 64), Some(B256::with_last_byte(64)));

        // Look up block 32
        assert_eq!(find_hash_by_number(&buffer, 32), Some(B256::with_last_byte(32)));

        // Look up the oldest (block 0)
        assert_eq!(find_hash_by_number(&buffer, 0), Some(B256::with_last_byte(0)));

        // Non-existent block number returns None
        assert_eq!(find_hash_by_number(&buffer, 100), None);
    }

    #[test]
    fn test_find_hash_by_number_insufficient_entries() {
        let mut buffer: AllocRingBuffer<BlockNumHash> = AllocRingBuffer::new(65);

        // With only 1 entry, only that block number works
        buffer.enqueue(BlockNumHash::new(1, B256::with_last_byte(1)));
        assert_eq!(find_hash_by_number(&buffer, 1), Some(B256::with_last_byte(1)));
        assert_eq!(find_hash_by_number(&buffer, 0), None);
        assert_eq!(find_hash_by_number(&buffer, 32), None);
        assert_eq!(find_hash_by_number(&buffer, 64), None);

        // With 33 entries (blocks 1..=33), block 1 works but block 64 doesn't
        for i in 2..=33u64 {
            buffer.enqueue(BlockNumHash::new(i, B256::with_last_byte(i as u8)));
        }
        assert_eq!(find_hash_by_number(&buffer, 1), Some(B256::with_last_byte(1)));
        assert_eq!(find_hash_by_number(&buffer, 64), None);
    }

    #[test]
    fn test_find_hash_by_number_with_gaps() {
        let mut buffer: AllocRingBuffer<BlockNumHash> = AllocRingBuffer::new(10);

        // Insert non-contiguous block numbers (simulating skipped blocks)
        buffer.enqueue(BlockNumHash::new(10, B256::with_last_byte(10)));
        buffer.enqueue(BlockNumHash::new(13, B256::with_last_byte(13)));
        buffer.enqueue(BlockNumHash::new(17, B256::with_last_byte(17)));

        // Existing block numbers are found
        assert_eq!(find_hash_by_number(&buffer, 10), Some(B256::with_last_byte(10)));
        assert_eq!(find_hash_by_number(&buffer, 13), Some(B256::with_last_byte(13)));
        assert_eq!(find_hash_by_number(&buffer, 17), Some(B256::with_last_byte(17)));

        // Skipped block numbers return None
        assert_eq!(find_hash_by_number(&buffer, 11), None);
        assert_eq!(find_hash_by_number(&buffer, 12), None);
        assert_eq!(find_hash_by_number(&buffer, 15), None);
    }

    /// Creates a test block with the given number and parent hash.
    fn test_block(number: u64, parent_hash: B256) -> reth_ethereum_primitives::Block {
        use reth_primitives_traits::{block::TestBlock, header::test_utils::TestHeader};
        let mut block = reth_ethereum_primitives::Block::default();
        block.header_mut().set_block_number(number);
        block.header_mut().set_parent_hash(parent_hash);
        block
    }

    fn num_hash(block: &reth_ethereum_primitives::Block) -> BlockNumHash {
        BlockNumHash::new(block.header().number(), block.header().hash_slow())
    }

    #[test]
    fn test_finalized_buffer_empty() {
        let mut buf = FinalizedBlockBuffer::<reth_ethereum_primitives::Block>::new();
        assert!(buf.is_empty());
        assert!(buf.drain_finalized(BlockNumHash::new(0, B256::ZERO)).is_empty());
    }

    #[test]
    fn test_finalized_buffer_drain_chain() {
        let mut buf = FinalizedBlockBuffer::new();

        let b1 = test_block(1, B256::ZERO);
        let b1_nh = num_hash(&b1);
        let b2 = test_block(2, b1_nh.hash);
        let b2_nh = num_hash(&b2);
        let b3 = test_block(3, b2_nh.hash);
        let b3_nh = num_hash(&b3);

        buf.insert(b1);
        buf.insert(b2);
        buf.insert(b3);
        assert_eq!(buf.len(), 3);

        // Finalize up to block 2 — should drain blocks 1 and 2
        let drained = buf.drain_finalized(b2_nh);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].header().number(), 1);
        assert_eq!(drained[1].header().number(), 2);

        // Block 3 remains
        assert_eq!(buf.len(), 1);

        // Finalize block 3
        let drained = buf.drain_finalized(b3_nh);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].header().number(), 3);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_finalized_buffer_prunes_sidechains() {
        let mut buf = FinalizedBlockBuffer::new();

        // Canonical chain: b1 -> b2
        let b1 = test_block(1, B256::ZERO);
        let b1_nh = num_hash(&b1);
        let b2 = test_block(2, b1_nh.hash);
        let b2_nh = num_hash(&b2);

        // Fork: b1 -> b2_fork (different block at height 2)
        let b2_fork = test_block(2, B256::with_last_byte(0xaa));
        let b2_fork_nh = num_hash(&b2_fork);
        assert_ne!(b2_nh.hash, b2_fork_nh.hash);

        // Block 3 on top of canonical b2
        let b3 = test_block(3, b2_nh.hash);

        buf.insert(b1);
        buf.insert(b2);
        buf.insert(b2_fork);
        buf.insert(b3);
        assert_eq!(buf.len(), 4);

        // Finalize canonical b2 — drains b1 and b2, prunes b2_fork (height <= 2)
        let drained = buf.drain_finalized(b2_nh);
        assert_eq!(drained.len(), 2);
        // Only b3 (height 3) remains, b2_fork was pruned
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn test_finalized_buffer_unknown_hash_no_change() {
        let mut buf = FinalizedBlockBuffer::new();
        buf.insert(test_block(1, B256::ZERO));

        // Unknown hash does not modify the buffer at all
        let drained = buf.drain_finalized(BlockNumHash::new(99, B256::with_last_byte(0xff)));
        assert!(drained.is_empty());
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn test_finalized_buffer_unknown_hash_keeps_higher_blocks() {
        let mut buf = FinalizedBlockBuffer::new();
        buf.insert(test_block(5, B256::ZERO));

        // Unknown hash does not modify the buffer at all
        let drained = buf.drain_finalized(BlockNumHash::new(3, B256::with_last_byte(0xff)));
        assert!(drained.is_empty());
        assert_eq!(buf.len(), 1);
    }
}
