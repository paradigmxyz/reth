use alloy_consensus::BlockHeader as _;
use alloy_primitives::{
    map::{B256HashSet, B256Map},
    BlockNumber, B256,
};
use futures::StreamExt;
use parking_lot::RwLock;
use reth_chain_state::ExecutedBlock;
use reth_ethereum_primitives::EthPrimitives;
use reth_node_api::{ConsensusEngineEvent, NodePrimitives};
use reth_primitives_traits::{Bytecode, RecoveredBlock};
use reth_storage_api::BlockNumReader;
use reth_tokio_util::EventStream;
use std::{collections::BTreeMap, sync::Arc};
use tracing::*;

/// Pending state for [`crate::RethRessProtocolProvider`].
#[derive(Clone, Default, Debug)]
pub struct PendingState<N: NodePrimitives>(Arc<RwLock<PendingStateInner<N>>>);

#[derive(Default, Debug)]
struct PendingStateInner<N: NodePrimitives> {
    blocks_by_hash: B256Map<ExecutedBlock<N>>,
    invalid_blocks_by_hash: B256Map<Arc<RecoveredBlock<N::Block>>>,
    block_hashes_by_number: BTreeMap<BlockNumber, B256HashSet>,
}

impl<N: NodePrimitives> PendingState<N> {
    /// Insert executed block with trie updates.
    pub fn insert_block(&self, block: ExecutedBlock<N>) {
        let mut this = self.0.write();
        let block_hash = block.recovered_block.hash();
        this.block_hashes_by_number
            .entry(block.recovered_block.number())
            .or_default()
            .insert(block_hash);
        this.blocks_by_hash.insert(block_hash, block);
    }

    /// Insert invalid block.
    pub fn insert_invalid_block(&self, block: Arc<RecoveredBlock<N::Block>>) {
        let mut this = self.0.write();
        let block_hash = block.hash();
        this.block_hashes_by_number.entry(block.number()).or_default().insert(block_hash);
        this.invalid_blocks_by_hash.insert(block_hash, block);
    }

    /// Returns only valid executed blocks by hash.
    pub fn executed_block(&self, hash: &B256) -> Option<ExecutedBlock<N>> {
        self.0.read().blocks_by_hash.get(hash).cloned()
    }

    /// Returns valid recovered block.
    pub fn recovered_block(&self, hash: &B256) -> Option<Arc<RecoveredBlock<N::Block>>> {
        self.executed_block(hash).map(|b| b.recovered_block)
    }

    /// Returns invalid recovered block.
    pub fn invalid_recovered_block(&self, hash: &B256) -> Option<Arc<RecoveredBlock<N::Block>>> {
        self.0.read().invalid_blocks_by_hash.get(hash).cloned()
    }

    /// Find bytecode in executed blocks state.
    pub fn find_bytecode(&self, code_hash: B256) -> Option<Bytecode> {
        let this = self.0.read();
        for block in this.blocks_by_hash.values() {
            if let Some(contract) = block.execution_output.bytecode(&code_hash) {
                return Some(contract);
            }
        }
        None
    }

    /// Remove all blocks before the specified block number.
    pub fn remove_before(&self, block_number: BlockNumber) -> u64 {
        let mut removed = 0;
        let mut this = self.0.write();
        while this
            .block_hashes_by_number
            .first_key_value()
            .is_some_and(|(number, _)| number <= &block_number)
        {
            let (_, block_hashes) = this.block_hashes_by_number.pop_first().unwrap();
            for block_hash in block_hashes {
                removed += 1;
                this.blocks_by_hash.remove(&block_hash);
                this.invalid_blocks_by_hash.remove(&block_hash);
            }
        }
        removed
    }
}

/// A task to maintain pending state based on consensus engine events.
pub async fn maintain_pending_state<P>(
    mut events: EventStream<ConsensusEngineEvent<EthPrimitives>>,
    provider: P,
    pending_state: PendingState<EthPrimitives>,
) where
    P: BlockNumReader,
{
    while let Some(event) = events.next().await {
        match event {
            ConsensusEngineEvent::CanonicalBlockAdded(block, _) |
            ConsensusEngineEvent::ForkBlockAdded(block, _) => {
                trace!(target: "reth::ress_provider", block = ? block.recovered_block().num_hash(), "Insert block into pending state");
                pending_state.insert_block(block);
            }
            ConsensusEngineEvent::InvalidBlock(block) => {
                if let Ok(block) = block.try_recover() {
                    trace!(target: "reth::ress_provider", block = ?block.num_hash(), "Insert invalid block into pending state");
                    pending_state.insert_invalid_block(Arc::new(block));
                }
            }
            ConsensusEngineEvent::ForkchoiceUpdated(state, status) => {
                if status.is_valid() {
                    let target = state.finalized_block_hash;
                    if let Ok(Some(block_number)) = provider.block_number(target) {
                        let count = pending_state.remove_before(block_number);
                        trace!(target: "reth::ress_provider", block_number, count, "Removing blocks before finalized");
                    }
                }
            }
            // ignore
            ConsensusEngineEvent::CanonicalChainCommitted(_, _) |
            ConsensusEngineEvent::BlockReceived(_) |
            ConsensusEngineEvent::LatestPersistedBlock(_) => (),
        }
    }
}
