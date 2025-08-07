//! Types for tracking the canonical chain state in memory.

use crate::{
    CanonStateNotification, CanonStateNotificationSender, CanonStateNotifications,
    ChainInfoTracker, MemoryOverlayStateProvider,
};
use alloy_consensus::{transaction::TransactionMeta, BlockHeader};
use alloy_eips::{BlockHashOrNumber, BlockNumHash};
use alloy_primitives::{map::HashMap, TxHash, B256};
use parking_lot::RwLock;
use reth_chainspec::ChainInfo;
use reth_ethereum_primitives::EthPrimitives;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_metrics::{metrics::Gauge, Metrics};
use reth_primitives_traits::{
    BlockBody as _, IndexedTx, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader,
    SignedTransaction,
};
use reth_storage_api::StateProviderBox;
use reth_trie::{updates::TrieUpdates, HashedPostState};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use tokio::sync::{broadcast, watch};

/// Size of the broadcast channel used to notify canonical state events.
const CANON_STATE_NOTIFICATION_CHANNEL_SIZE: usize = 256;

/// Metrics for the in-memory state.
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree.in_mem_state")]
pub(crate) struct InMemoryStateMetrics {
    /// The block number of the earliest block in the in-memory state.
    pub(crate) earliest_block: Gauge,
    /// The block number of the latest block in the in-memory state.
    pub(crate) latest_block: Gauge,
    /// The number of blocks in the in-memory state.
    pub(crate) num_blocks: Gauge,
}

/// Container type for in memory state data of the canonical chain.
///
/// This tracks blocks and their state that haven't been persisted to disk yet but are part of the
/// canonical chain that can be traced back to a canonical block on disk.
///
/// # Locking behavior on state updates
///
/// All update calls must be atomic, meaning that they must acquire all locks at once, before
/// modifying the state. This is to ensure that the internal state is always consistent.
/// Update functions ensure that the numbers write lock is always acquired first, because lookup by
/// numbers first read the numbers map and then the blocks map.
/// By acquiring the numbers lock first, we ensure that read-only lookups don't deadlock updates.
/// This holds, because only lookup by number functions need to acquire the numbers lock first to
/// get the block hash.
#[derive(Debug, Default)]
pub(crate) struct InMemoryState<N: NodePrimitives = EthPrimitives> {
    /// All canonical blocks that are not on disk yet.
    blocks: RwLock<HashMap<B256, Arc<BlockState<N>>>>,
    /// Mapping of block numbers to block hashes.
    numbers: RwLock<BTreeMap<u64, B256>>,
    /// The pending block that has not yet been made canonical.
    pending: watch::Sender<Option<BlockState<N>>>,
    /// Metrics for the in-memory state.
    metrics: InMemoryStateMetrics,
}

impl<N: NodePrimitives> InMemoryState<N> {
    pub(crate) fn new(
        blocks: HashMap<B256, Arc<BlockState<N>>>,
        numbers: BTreeMap<u64, B256>,
        pending: Option<BlockState<N>>,
    ) -> Self {
        let (pending, _) = watch::channel(pending);
        let this = Self {
            blocks: RwLock::new(blocks),
            numbers: RwLock::new(numbers),
            pending,
            metrics: Default::default(),
        };
        this.update_metrics();
        this
    }

    /// Update the metrics for the in-memory state.
    ///
    /// # Locking behavior
    ///
    /// This tries to acquire a read lock. Drop any write locks before calling this.
    pub(crate) fn update_metrics(&self) {
        let numbers = self.numbers.read();
        if let Some((earliest_block_number, _)) = numbers.first_key_value() {
            self.metrics.earliest_block.set(*earliest_block_number as f64);
        }
        if let Some((latest_block_number, _)) = numbers.last_key_value() {
            self.metrics.latest_block.set(*latest_block_number as f64);
        }
        self.metrics.num_blocks.set(numbers.len() as f64);
    }

    /// Returns the state for a given block hash.
    pub(crate) fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.blocks.read().get(&hash).cloned()
    }

    /// Returns the state for a given block number.
    pub(crate) fn state_by_number(&self, number: u64) -> Option<Arc<BlockState<N>>> {
        let hash = self.hash_by_number(number)?;
        self.state_by_hash(hash)
    }

    /// Returns the hash for a specific block number
    pub(crate) fn hash_by_number(&self, number: u64) -> Option<B256> {
        self.numbers.read().get(&number).copied()
    }

    /// Returns the current chain head state.
    pub(crate) fn head_state(&self) -> Option<Arc<BlockState<N>>> {
        let hash = *self.numbers.read().last_key_value()?.1;
        self.state_by_hash(hash)
    }

    /// Returns the pending state corresponding to the current head plus one,
    /// from the payload received in newPayload that does not have a FCU yet.
    pub(crate) fn pending_state(&self) -> Option<BlockState<N>> {
        self.pending.borrow().clone()
    }

    #[cfg(test)]
    fn block_count(&self) -> usize {
        self.blocks.read().len()
    }
}

/// Inner type to provide in memory state. It includes a chain tracker to be
/// advanced internally by the tree.
#[derive(Debug)]
pub(crate) struct CanonicalInMemoryStateInner<N: NodePrimitives> {
    /// Tracks certain chain information, such as the canonical head, safe head, and finalized
    /// head.
    pub(crate) chain_info_tracker: ChainInfoTracker<N>,
    /// Tracks blocks at the tip of the chain that have not been persisted to disk yet.
    pub(crate) in_memory_state: InMemoryState<N>,
    /// A broadcast stream that emits events when the canonical chain is updated.
    pub(crate) canon_state_notification_sender: CanonStateNotificationSender<N>,
}

impl<N: NodePrimitives> CanonicalInMemoryStateInner<N> {
    /// Clears all entries in the in memory state.
    fn clear(&self) {
        {
            // acquire locks, starting with the numbers lock
            let mut numbers = self.in_memory_state.numbers.write();
            let mut blocks = self.in_memory_state.blocks.write();
            numbers.clear();
            blocks.clear();
            self.in_memory_state.pending.send_modify(|p| {
                p.take();
            });
        }
        self.in_memory_state.update_metrics();
    }
}

type PendingBlockAndReceipts<N> =
    (RecoveredBlock<<N as NodePrimitives>::Block>, Vec<reth_primitives_traits::ReceiptTy<N>>);

/// This type is responsible for providing the blocks, receipts, and state for
/// all canonical blocks not on disk yet and keeps track of the block range that
/// is in memory.
#[derive(Debug, Clone)]
pub struct CanonicalInMemoryState<N: NodePrimitives = EthPrimitives> {
    pub(crate) inner: Arc<CanonicalInMemoryStateInner<N>>,
}

impl<N: NodePrimitives> CanonicalInMemoryState<N> {
    /// Create a new in-memory state with the given blocks, numbers, pending state, and optional
    /// finalized header.
    pub fn new(
        blocks: HashMap<B256, Arc<BlockState<N>>>,
        numbers: BTreeMap<u64, B256>,
        pending: Option<BlockState<N>>,
        finalized: Option<SealedHeader<N::BlockHeader>>,
        safe: Option<SealedHeader<N::BlockHeader>>,
    ) -> Self {
        let in_memory_state = InMemoryState::new(blocks, numbers, pending);
        let header = in_memory_state.head_state().map_or_else(SealedHeader::default, |state| {
            state.block_ref().recovered_block().clone_sealed_header()
        });
        let chain_info_tracker = ChainInfoTracker::new(header, finalized, safe);
        let (canon_state_notification_sender, _) =
            broadcast::channel(CANON_STATE_NOTIFICATION_CHANNEL_SIZE);

        Self {
            inner: Arc::new(CanonicalInMemoryStateInner {
                chain_info_tracker,
                in_memory_state,
                canon_state_notification_sender,
            }),
        }
    }

    /// Create an empty state.
    pub fn empty() -> Self {
        Self::new(HashMap::default(), BTreeMap::new(), None, None, None)
    }

    /// Create a new in memory state with the given local head and finalized header
    /// if it exists.
    pub fn with_head(
        head: SealedHeader<N::BlockHeader>,
        finalized: Option<SealedHeader<N::BlockHeader>>,
        safe: Option<SealedHeader<N::BlockHeader>>,
    ) -> Self {
        let chain_info_tracker = ChainInfoTracker::new(head, finalized, safe);
        let in_memory_state = InMemoryState::default();
        let (canon_state_notification_sender, _) =
            broadcast::channel(CANON_STATE_NOTIFICATION_CHANNEL_SIZE);
        let inner = CanonicalInMemoryStateInner {
            chain_info_tracker,
            in_memory_state,
            canon_state_notification_sender,
        };

        Self { inner: Arc::new(inner) }
    }

    /// Returns the block hash corresponding to the given number.
    pub fn hash_by_number(&self, number: u64) -> Option<B256> {
        self.inner.in_memory_state.hash_by_number(number)
    }

    /// Returns the header corresponding to the given hash.
    pub fn header_by_hash(&self, hash: B256) -> Option<SealedHeader<N::BlockHeader>> {
        self.state_by_hash(hash)
            .map(|block| block.block_ref().recovered_block().clone_sealed_header())
    }

    /// Clears all entries in the in memory state.
    pub fn clear_state(&self) {
        self.inner.clear()
    }

    /// Updates the pending block with the given block.
    ///
    /// Note: This assumes that the parent block of the pending block is canonical.
    pub fn set_pending_block(&self, pending: ExecutedBlockWithTrieUpdates<N>) {
        // fetch the state of the pending block's parent block
        let parent = self.state_by_hash(pending.recovered_block().parent_hash());
        let pending = BlockState::with_parent(pending, parent);
        self.inner.in_memory_state.pending.send_modify(|p| {
            p.replace(pending);
        });
        self.inner.in_memory_state.update_metrics();
    }

    /// Append new blocks to the in memory state.
    ///
    /// This removes all reorged blocks and appends the new blocks to the tracked chain and connects
    /// them to their parent blocks.
    fn update_blocks<I, R>(&self, new_blocks: I, reorged: R)
    where
        I: IntoIterator<Item = ExecutedBlockWithTrieUpdates<N>>,
        R: IntoIterator<Item = ExecutedBlock<N>>,
    {
        {
            // acquire locks, starting with the numbers lock
            let mut numbers = self.inner.in_memory_state.numbers.write();
            let mut blocks = self.inner.in_memory_state.blocks.write();

            // we first remove the blocks from the reorged chain
            for block in reorged {
                let hash = block.recovered_block().hash();
                let number = block.recovered_block().number();
                blocks.remove(&hash);
                numbers.remove(&number);
            }

            // insert the new blocks
            for block in new_blocks {
                let parent = blocks.get(&block.recovered_block().parent_hash()).cloned();
                let block_state = BlockState::with_parent(block, parent);
                let hash = block_state.hash();
                let number = block_state.number();

                // append new blocks
                blocks.insert(hash, Arc::new(block_state));
                numbers.insert(number, hash);
            }

            // remove the pending state
            self.inner.in_memory_state.pending.send_modify(|p| {
                p.take();
            });
        }
        self.inner.in_memory_state.update_metrics();
    }

    /// Update the in memory state with the given chain update.
    pub fn update_chain(&self, new_chain: NewCanonicalChain<N>) {
        match new_chain {
            NewCanonicalChain::Commit { new } => {
                self.update_blocks(new, vec![]);
            }
            NewCanonicalChain::Reorg { new, old } => {
                self.update_blocks(new, old);
            }
        }
    }

    /// Removes blocks from the in memory state that are persisted to the given height.
    ///
    /// This will update the links between blocks and remove all blocks that are [..
    /// `persisted_height`].
    pub fn remove_persisted_blocks(&self, persisted_num_hash: BlockNumHash) {
        // if the persisted hash is not in the canonical in memory state, do nothing, because it
        // means canonical blocks were not actually persisted.
        //
        // This can happen if the persistence task takes a long time, while a reorg is happening.
        {
            if self.inner.in_memory_state.blocks.read().get(&persisted_num_hash.hash).is_none() {
                // do nothing
                return
            }
        }

        {
            // acquire locks, starting with the numbers lock
            let mut numbers = self.inner.in_memory_state.numbers.write();
            let mut blocks = self.inner.in_memory_state.blocks.write();

            let BlockNumHash { number: persisted_height, hash: _ } = persisted_num_hash;

            // clear all numbers
            numbers.clear();

            // drain all blocks and only keep the ones that are not persisted (below the persisted
            // height)
            let mut old_blocks = blocks
                .drain()
                .filter(|(_, b)| b.block_ref().recovered_block().number() > persisted_height)
                .map(|(_, b)| b.block.clone())
                .collect::<Vec<_>>();

            // sort the blocks by number so we can insert them back in natural order (low -> high)
            old_blocks.sort_unstable_by_key(|block| block.recovered_block().number());

            // re-insert the blocks in natural order and connect them to their parent blocks
            for block in old_blocks {
                let parent = blocks.get(&block.recovered_block().parent_hash()).cloned();
                let block_state = BlockState::with_parent(block, parent);
                let hash = block_state.hash();
                let number = block_state.number();

                // append new blocks
                blocks.insert(hash, Arc::new(block_state));
                numbers.insert(number, hash);
            }

            // also shift the pending state if it exists
            self.inner.in_memory_state.pending.send_modify(|p| {
                if let Some(p) = p.as_mut() {
                    p.parent = blocks.get(&p.block_ref().recovered_block().parent_hash()).cloned();
                }
            });
        }
        self.inner.in_memory_state.update_metrics();
    }

    /// Returns in memory state corresponding the given hash.
    pub fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.state_by_hash(hash)
    }

    /// Returns in memory state corresponding the block number.
    pub fn state_by_number(&self, number: u64) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.state_by_number(number)
    }

    /// Returns the in memory head state.
    pub fn head_state(&self) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.head_state()
    }

    /// Returns the in memory pending state.
    pub fn pending_state(&self) -> Option<BlockState<N>> {
        self.inner.in_memory_state.pending_state()
    }

    /// Returns the in memory pending `BlockNumHash`.
    pub fn pending_block_num_hash(&self) -> Option<BlockNumHash> {
        self.inner
            .in_memory_state
            .pending_state()
            .map(|state| BlockNumHash { number: state.number(), hash: state.hash() })
    }

    /// Returns the current `ChainInfo`.
    pub fn chain_info(&self) -> ChainInfo {
        self.inner.chain_info_tracker.chain_info()
    }

    /// Returns the latest canonical block number.
    pub fn get_canonical_block_number(&self) -> u64 {
        self.inner.chain_info_tracker.get_canonical_block_number()
    }

    /// Returns the `BlockNumHash` of the safe head.
    pub fn get_safe_num_hash(&self) -> Option<BlockNumHash> {
        self.inner.chain_info_tracker.get_safe_num_hash()
    }

    /// Returns the `BlockNumHash` of the finalized head.
    pub fn get_finalized_num_hash(&self) -> Option<BlockNumHash> {
        self.inner.chain_info_tracker.get_finalized_num_hash()
    }

    /// Hook for new fork choice update.
    pub fn on_forkchoice_update_received(&self) {
        self.inner.chain_info_tracker.on_forkchoice_update_received();
    }

    /// Returns the timestamp of the last received update.
    pub fn last_received_update_timestamp(&self) -> Option<Instant> {
        self.inner.chain_info_tracker.last_forkchoice_update_received_at()
    }

    /// Canonical head setter.
    pub fn set_canonical_head(&self, header: SealedHeader<N::BlockHeader>) {
        self.inner.chain_info_tracker.set_canonical_head(header);
    }

    /// Safe head setter.
    pub fn set_safe(&self, header: SealedHeader<N::BlockHeader>) {
        self.inner.chain_info_tracker.set_safe(header);
    }

    /// Finalized head setter.
    pub fn set_finalized(&self, header: SealedHeader<N::BlockHeader>) {
        self.inner.chain_info_tracker.set_finalized(header);
    }

    /// Canonical head getter.
    pub fn get_canonical_head(&self) -> SealedHeader<N::BlockHeader> {
        self.inner.chain_info_tracker.get_canonical_head()
    }

    /// Finalized header getter.
    pub fn get_finalized_header(&self) -> Option<SealedHeader<N::BlockHeader>> {
        self.inner.chain_info_tracker.get_finalized_header()
    }

    /// Safe header getter.
    pub fn get_safe_header(&self) -> Option<SealedHeader<N::BlockHeader>> {
        self.inner.chain_info_tracker.get_safe_header()
    }

    /// Returns the `SealedHeader` corresponding to the pending state.
    pub fn pending_sealed_header(&self) -> Option<SealedHeader<N::BlockHeader>> {
        self.pending_state().map(|h| h.block_ref().recovered_block().clone_sealed_header())
    }

    /// Returns the `Header` corresponding to the pending state.
    pub fn pending_header(&self) -> Option<N::BlockHeader> {
        self.pending_sealed_header().map(|sealed_header| sealed_header.unseal())
    }

    /// Returns the `SealedBlock` corresponding to the pending state.
    pub fn pending_block(&self) -> Option<SealedBlock<N::Block>> {
        self.pending_state()
            .map(|block_state| block_state.block_ref().recovered_block().sealed_block().clone())
    }

    /// Returns the `RecoveredBlock` corresponding to the pending state.
    pub fn pending_recovered_block(&self) -> Option<RecoveredBlock<N::Block>>
    where
        N::SignedTx: SignedTransaction,
    {
        self.pending_state().map(|block_state| block_state.block_ref().recovered_block().clone())
    }

    /// Returns a tuple with the `SealedBlock` corresponding to the pending
    /// state and a vector of its `Receipt`s.
    pub fn pending_block_and_receipts(&self) -> Option<PendingBlockAndReceipts<N>> {
        self.pending_state().map(|block_state| {
            (
                block_state.block_ref().recovered_block().clone(),
                block_state.executed_block_receipts(),
            )
        })
    }

    /// Subscribe to new blocks events.
    pub fn subscribe_canon_state(&self) -> CanonStateNotifications<N> {
        self.inner.canon_state_notification_sender.subscribe()
    }

    /// Subscribe to new safe block events.
    pub fn subscribe_safe_block(&self) -> watch::Receiver<Option<SealedHeader<N::BlockHeader>>> {
        self.inner.chain_info_tracker.subscribe_safe_block()
    }

    /// Subscribe to new finalized block events.
    pub fn subscribe_finalized_block(
        &self,
    ) -> watch::Receiver<Option<SealedHeader<N::BlockHeader>>> {
        self.inner.chain_info_tracker.subscribe_finalized_block()
    }

    /// Attempts to send a new [`CanonStateNotification`] to all active Receiver handles.
    pub fn notify_canon_state(&self, event: CanonStateNotification<N>) {
        self.inner.canon_state_notification_sender.send(event).ok();
    }

    /// Return state provider with reference to in-memory blocks that overlay database state.
    ///
    /// This merges the state of all blocks that are part of the chain that the requested block is
    /// the head of. This includes all blocks that connect back to the canonical block on disk.
    pub fn state_provider(
        &self,
        hash: B256,
        historical: StateProviderBox,
    ) -> MemoryOverlayStateProvider<N> {
        let in_memory = if let Some(state) = self.state_by_hash(hash) {
            state.chain().map(|block_state| block_state.block()).collect()
        } else {
            Vec::new()
        };

        MemoryOverlayStateProvider::new(historical, in_memory)
    }

    /// Returns an iterator over all __canonical blocks__ in the in-memory state, from newest to
    /// oldest (highest to lowest).
    ///
    /// This iterator contains a snapshot of the in-memory state at the time of the call.
    pub fn canonical_chain(&self) -> impl Iterator<Item = Arc<BlockState<N>>> {
        self.inner.in_memory_state.head_state().into_iter().flat_map(|head| head.iter())
    }

    /// Returns [`SignedTransaction`] type for the given `TxHash` if found.
    pub fn transaction_by_hash(&self, hash: TxHash) -> Option<N::SignedTx> {
        for block_state in self.canonical_chain() {
            if let Some(tx) =
                block_state.block_ref().recovered_block().body().transaction_by_hash(&hash)
            {
                return Some(tx.clone())
            }
        }
        None
    }

    /// Returns a tuple with [`SignedTransaction`] type and [`TransactionMeta`] for the
    /// given [`TxHash`] if found.
    pub fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> Option<(N::SignedTx, TransactionMeta)> {
        for block_state in self.canonical_chain() {
            if let Some(indexed) = block_state.find_indexed(tx_hash) {
                return Some((indexed.tx().clone(), indexed.meta()));
            }
        }
        None
    }
}

/// State after applying the given block, this block is part of the canonical chain that partially
/// stored in memory and can be traced back to a canonical block on disk.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BlockState<N: NodePrimitives = EthPrimitives> {
    /// The executed block that determines the state after this block has been executed.
    block: ExecutedBlockWithTrieUpdates<N>,
    /// The block's parent block if it exists.
    parent: Option<Arc<BlockState<N>>>,
}

impl<N: NodePrimitives> BlockState<N> {
    /// [`BlockState`] constructor.
    pub const fn new(block: ExecutedBlockWithTrieUpdates<N>) -> Self {
        Self { block, parent: None }
    }

    /// [`BlockState`] constructor with parent.
    pub const fn with_parent(
        block: ExecutedBlockWithTrieUpdates<N>,
        parent: Option<Arc<Self>>,
    ) -> Self {
        Self { block, parent }
    }

    /// Returns the hash and block of the on disk block this state can be traced back to.
    pub fn anchor(&self) -> BlockNumHash {
        if let Some(parent) = &self.parent {
            parent.anchor()
        } else {
            self.block.recovered_block().parent_num_hash()
        }
    }

    /// Returns the executed block that determines the state.
    pub fn block(&self) -> ExecutedBlockWithTrieUpdates<N> {
        self.block.clone()
    }

    /// Returns a reference to the executed block that determines the state.
    pub const fn block_ref(&self) -> &ExecutedBlockWithTrieUpdates<N> {
        &self.block
    }

    /// Returns the hash of executed block that determines the state.
    pub fn hash(&self) -> B256 {
        self.block.recovered_block().hash()
    }

    /// Returns the block number of executed block that determines the state.
    pub fn number(&self) -> u64 {
        self.block.recovered_block().number()
    }

    /// Returns the state root after applying the executed block that determines
    /// the state.
    pub fn state_root(&self) -> B256 {
        self.block.recovered_block().state_root()
    }

    /// Returns the `Receipts` of executed block that determines the state.
    pub fn receipts(&self) -> &Vec<Vec<N::Receipt>> {
        &self.block.execution_outcome().receipts
    }

    /// Returns a vector of `Receipt` of executed block that determines the state.
    /// We assume that the `Receipts` in the executed block `ExecutionOutcome`
    /// has only one element corresponding to the executed block associated to
    /// the state.
    pub fn executed_block_receipts(&self) -> Vec<N::Receipt> {
        let receipts = self.receipts();

        debug_assert!(
            receipts.len() <= 1,
            "Expected at most one block's worth of receipts, found {}",
            receipts.len()
        );

        receipts.first().cloned().unwrap_or_default()
    }

    /// Returns a vector of __parent__ `BlockStates`.
    ///
    /// The block state order in the output vector is newest to oldest (highest to lowest):
    /// `[5,4,3,2,1]`
    ///
    /// Note: This does not include self.
    pub fn parent_state_chain(&self) -> Vec<&Self> {
        let mut parents = Vec::new();
        let mut current = self.parent.as_deref();

        while let Some(parent) = current {
            parents.push(parent);
            current = parent.parent.as_deref();
        }

        parents
    }

    /// Returns a vector of `BlockStates` representing the entire in memory chain.
    /// The block state order in the output vector is newest to oldest (highest to lowest),
    /// including self as the first element.
    pub fn chain(&self) -> impl Iterator<Item = &Self> {
        std::iter::successors(Some(self), |state| state.parent.as_deref())
    }

    /// Appends the parent chain of this [`BlockState`] to the given vector.
    pub fn append_parent_chain<'a>(&'a self, chain: &mut Vec<&'a Self>) {
        chain.extend(self.parent_state_chain());
    }

    /// Returns an iterator over the atomically captured chain of in memory blocks.
    ///
    /// This yields the blocks from newest to oldest (highest to lowest).
    pub fn iter(self: Arc<Self>) -> impl Iterator<Item = Arc<Self>> {
        std::iter::successors(Some(self), |state| state.parent.clone())
    }

    /// Return state provider with reference to in-memory blocks that overlay database state.
    ///
    /// This merges the state of all blocks that are part of the chain that the this block is
    /// the head of. This includes all blocks that connect back to the canonical block on disk.
    pub fn state_provider(&self, historical: StateProviderBox) -> MemoryOverlayStateProvider<N> {
        let in_memory = self.chain().map(|block_state| block_state.block()).collect();

        MemoryOverlayStateProvider::new(historical, in_memory)
    }

    /// Tries to find a block by [`BlockHashOrNumber`] in the chain ending at this block.
    pub fn block_on_chain(&self, hash_or_num: BlockHashOrNumber) -> Option<&Self> {
        self.chain().find(|block| match hash_or_num {
            BlockHashOrNumber::Hash(hash) => block.hash() == hash,
            BlockHashOrNumber::Number(number) => block.number() == number,
        })
    }

    /// Tries to find a transaction by [`TxHash`] in the chain ending at this block.
    pub fn transaction_on_chain(&self, hash: TxHash) -> Option<N::SignedTx> {
        self.chain().find_map(|block_state| {
            block_state.block_ref().recovered_block().body().transaction_by_hash(&hash).cloned()
        })
    }

    /// Tries to find a transaction with meta by [`TxHash`] in the chain ending at this block.
    pub fn transaction_meta_on_chain(
        &self,
        tx_hash: TxHash,
    ) -> Option<(N::SignedTx, TransactionMeta)> {
        self.chain().find_map(|block_state| {
            block_state.find_indexed(tx_hash).map(|indexed| (indexed.tx().clone(), indexed.meta()))
        })
    }

    /// Finds a transaction by hash and returns it with its index and block context.
    pub fn find_indexed(&self, tx_hash: TxHash) -> Option<IndexedTx<'_, N::Block>> {
        self.block_ref().recovered_block().find_indexed(tx_hash)
    }
}

/// Represents an executed block stored in-memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutedBlock<N: NodePrimitives = EthPrimitives> {
    /// Recovered Block
    pub recovered_block: Arc<RecoveredBlock<N::Block>>,
    /// Block's execution outcome.
    pub execution_output: Arc<ExecutionOutcome<N::Receipt>>,
    /// Block's hashed state.
    pub hashed_state: Arc<HashedPostState>,
}

impl<N: NodePrimitives> Default for ExecutedBlock<N> {
    fn default() -> Self {
        Self {
            recovered_block: Default::default(),
            execution_output: Default::default(),
            hashed_state: Default::default(),
        }
    }
}

impl<N: NodePrimitives> ExecutedBlock<N> {
    /// Returns a reference to an inner [`SealedBlock`]
    #[inline]
    pub fn sealed_block(&self) -> &SealedBlock<N::Block> {
        self.recovered_block.sealed_block()
    }

    /// Returns a reference to [`RecoveredBlock`]
    #[inline]
    pub fn recovered_block(&self) -> &RecoveredBlock<N::Block> {
        &self.recovered_block
    }

    /// Returns a reference to the block's execution outcome
    #[inline]
    pub fn execution_outcome(&self) -> &ExecutionOutcome<N::Receipt> {
        &self.execution_output
    }

    /// Returns a reference to the hashed state result of the execution outcome
    #[inline]
    pub fn hashed_state(&self) -> &HashedPostState {
        &self.hashed_state
    }
}

/// Trie updates that result from calculating the state root for the block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutedTrieUpdates {
    /// Trie updates present. State root was calculated, and the trie updates can be applied to the
    /// database.
    Present(Arc<TrieUpdates>),
    /// Trie updates missing. State root was calculated, but the trie updates cannot be applied to
    /// the current database state. To apply the updates, the state root must be recalculated, and
    /// new trie updates must be generated.
    ///
    /// This can happen when processing fork chain blocks that are building on top of the
    /// historical database state. Since we don't store the historical trie state, we cannot
    /// generate the trie updates for it.
    Missing,
}

impl ExecutedTrieUpdates {
    /// Creates a [`ExecutedTrieUpdates`] with present but empty trie updates.
    pub fn empty() -> Self {
        Self::Present(Arc::default())
    }

    /// Sets the trie updates to the provided value as present.
    pub fn set_present(&mut self, updates: Arc<TrieUpdates>) {
        *self = Self::Present(updates);
    }

    /// Takes the present trie updates, leaving the state as missing.
    pub fn take_present(&mut self) -> Option<Arc<TrieUpdates>> {
        match self {
            Self::Present(updates) => {
                let updates = core::mem::take(updates);
                *self = Self::Missing;
                Some(updates)
            }
            Self::Missing => None,
        }
    }

    /// Returns a reference to the trie updates if present.
    #[allow(clippy::missing_const_for_fn)] // false positive
    pub fn as_ref(&self) -> Option<&TrieUpdates> {
        match self {
            Self::Present(updates) => Some(updates),
            Self::Missing => None,
        }
    }

    /// Returns `true` if the trie updates are present.
    pub const fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }

    /// Returns `true` if the trie updates are missing.
    pub const fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

/// An [`ExecutedBlock`] with its [`TrieUpdates`].
///
/// We store it as separate type because [`TrieUpdates`] are only available for blocks stored in
/// memory and can't be obtained for canonical persisted blocks.
#[derive(
    Clone, Debug, PartialEq, Eq, derive_more::Deref, derive_more::DerefMut, derive_more::Into,
)]
pub struct ExecutedBlockWithTrieUpdates<N: NodePrimitives = EthPrimitives> {
    /// Inner [`ExecutedBlock`].
    #[deref]
    #[deref_mut]
    #[into]
    pub block: ExecutedBlock<N>,
    /// Trie updates that result from calculating the state root for the block.
    ///
    /// If [`ExecutedTrieUpdates::Missing`], the trie updates should be computed when persisting
    /// the block **on top of the canonical parent**.
    pub trie: ExecutedTrieUpdates,
}

impl<N: NodePrimitives> ExecutedBlockWithTrieUpdates<N> {
    /// [`ExecutedBlock`] constructor.
    pub const fn new(
        recovered_block: Arc<RecoveredBlock<N::Block>>,
        execution_output: Arc<ExecutionOutcome<N::Receipt>>,
        hashed_state: Arc<HashedPostState>,
        trie: ExecutedTrieUpdates,
    ) -> Self {
        Self { block: ExecutedBlock { recovered_block, execution_output, hashed_state }, trie }
    }

    /// Returns a reference to the trie updates for the block, if present.
    #[inline]
    pub fn trie_updates(&self) -> Option<&TrieUpdates> {
        self.trie.as_ref()
    }

    /// Converts the value into [`SealedBlock`].
    pub fn into_sealed_block(self) -> SealedBlock<N::Block> {
        let block = Arc::unwrap_or_clone(self.block.recovered_block);
        block.into_sealed_block()
    }
}

/// Non-empty chain of blocks.
#[derive(Debug)]
pub enum NewCanonicalChain<N: NodePrimitives = EthPrimitives> {
    /// A simple append to the current canonical head
    Commit {
        /// all blocks that lead back to the canonical head
        new: Vec<ExecutedBlockWithTrieUpdates<N>>,
    },
    /// A reorged chain consists of two chains that trace back to a shared ancestor block at which
    /// point they diverge.
    Reorg {
        /// All blocks of the _new_ chain
        new: Vec<ExecutedBlockWithTrieUpdates<N>>,
        /// All blocks of the _old_ chain
        ///
        /// These are not [`ExecutedBlockWithTrieUpdates`] because we don't always have the trie
        /// updates for the old canonical chain. For example, in case of node being restarted right
        /// before the reorg [`TrieUpdates`] can't be fetched from database.
        old: Vec<ExecutedBlock<N>>,
    },
}

impl<N: NodePrimitives<SignedTx: SignedTransaction>> NewCanonicalChain<N> {
    /// Returns the length of the new chain.
    pub fn new_block_count(&self) -> usize {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => new.len(),
        }
    }

    /// Returns the length of the reorged chain.
    pub fn reorged_block_count(&self) -> usize {
        match self {
            Self::Commit { .. } => 0,
            Self::Reorg { old, .. } => old.len(),
        }
    }

    /// Converts the new chain into a notification that will be emitted to listeners
    pub fn to_chain_notification(&self) -> CanonStateNotification<N> {
        match self {
            Self::Commit { new } => {
                let new = Arc::new(new.iter().fold(Chain::default(), |mut chain, exec| {
                    chain.append_block(
                        exec.recovered_block().clone(),
                        exec.execution_outcome().clone(),
                    );
                    chain
                }));
                CanonStateNotification::Commit { new }
            }
            Self::Reorg { new, old } => {
                let new = Arc::new(new.iter().fold(Chain::default(), |mut chain, exec| {
                    chain.append_block(
                        exec.recovered_block().clone(),
                        exec.execution_outcome().clone(),
                    );
                    chain
                }));
                let old = Arc::new(old.iter().fold(Chain::default(), |mut chain, exec| {
                    chain.append_block(
                        exec.recovered_block().clone(),
                        exec.execution_outcome().clone(),
                    );
                    chain
                }));
                CanonStateNotification::Reorg { new, old }
            }
        }
    }

    /// Returns the new tip of the chain.
    ///
    /// Returns the new tip for [`Self::Reorg`] and [`Self::Commit`] variants which commit at least
    /// 1 new block.
    pub fn tip(&self) -> &SealedBlock<N::Block> {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => {
                new.last().expect("non empty blocks").recovered_block()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestBlockBuilder;
    use alloy_eips::eip7685::Requests;
    use alloy_primitives::{Address, BlockNumber, Bytes, StorageKey, StorageValue};
    use rand::Rng;
    use reth_errors::ProviderResult;
    use reth_ethereum_primitives::{EthPrimitives, Receipt};
    use reth_primitives_traits::{Account, Bytecode};
    use reth_storage_api::{
        AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider,
        StateProofProvider, StateProvider, StateRootProvider, StorageRootProvider,
    };
    use reth_trie::{
        AccountProof, HashedStorage, MultiProof, MultiProofTargets, StorageMultiProof,
        StorageProof, TrieInput,
    };

    fn create_mock_state(
        test_block_builder: &mut TestBlockBuilder<EthPrimitives>,
        block_number: u64,
        parent_hash: B256,
    ) -> BlockState {
        BlockState::new(
            test_block_builder.get_executed_block_with_number(block_number, parent_hash),
        )
    }

    fn create_mock_state_chain(
        test_block_builder: &mut TestBlockBuilder<EthPrimitives>,
        num_blocks: u64,
    ) -> Vec<BlockState> {
        let mut chain = Vec::with_capacity(num_blocks as usize);
        let mut parent_hash = B256::random();
        let mut parent_state: Option<BlockState> = None;

        for i in 1..=num_blocks {
            let mut state = create_mock_state(test_block_builder, i, parent_hash);
            if let Some(parent) = parent_state {
                state.parent = Some(Arc::new(parent));
            }
            parent_hash = state.hash();
            parent_state = Some(state.clone());
            chain.push(state);
        }

        chain
    }

    struct MockStateProvider;

    impl StateProvider for MockStateProvider {
        fn storage(
            &self,
            _address: Address,
            _storage_key: StorageKey,
        ) -> ProviderResult<Option<StorageValue>> {
            Ok(None)
        }
    }

    impl BytecodeReader for MockStateProvider {
        fn bytecode_by_hash(&self, _code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
            Ok(None)
        }
    }

    impl BlockHashReader for MockStateProvider {
        fn block_hash(&self, _number: BlockNumber) -> ProviderResult<Option<B256>> {
            Ok(None)
        }

        fn canonical_hashes_range(
            &self,
            _start: BlockNumber,
            _end: BlockNumber,
        ) -> ProviderResult<Vec<B256>> {
            Ok(vec![])
        }
    }

    impl AccountReader for MockStateProvider {
        fn basic_account(&self, _address: &Address) -> ProviderResult<Option<Account>> {
            Ok(None)
        }
    }

    impl StateRootProvider for MockStateProvider {
        fn state_root(&self, _hashed_state: HashedPostState) -> ProviderResult<B256> {
            Ok(B256::random())
        }

        fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
            Ok(B256::random())
        }

        fn state_root_with_updates(
            &self,
            _hashed_state: HashedPostState,
        ) -> ProviderResult<(B256, TrieUpdates)> {
            Ok((B256::random(), TrieUpdates::default()))
        }

        fn state_root_from_nodes_with_updates(
            &self,
            _input: TrieInput,
        ) -> ProviderResult<(B256, TrieUpdates)> {
            Ok((B256::random(), TrieUpdates::default()))
        }
    }

    impl HashedPostStateProvider for MockStateProvider {
        fn hashed_post_state(&self, _bundle_state: &revm_database::BundleState) -> HashedPostState {
            HashedPostState::default()
        }
    }

    impl StorageRootProvider for MockStateProvider {
        fn storage_root(
            &self,
            _address: Address,
            _hashed_storage: HashedStorage,
        ) -> ProviderResult<B256> {
            Ok(B256::random())
        }

        fn storage_proof(
            &self,
            _address: Address,
            slot: B256,
            _hashed_storage: HashedStorage,
        ) -> ProviderResult<StorageProof> {
            Ok(StorageProof::new(slot))
        }

        fn storage_multiproof(
            &self,
            _address: Address,
            _slots: &[B256],
            _hashed_storage: HashedStorage,
        ) -> ProviderResult<StorageMultiProof> {
            Ok(StorageMultiProof::empty())
        }
    }

    impl StateProofProvider for MockStateProvider {
        fn proof(
            &self,
            _input: TrieInput,
            _address: Address,
            _slots: &[B256],
        ) -> ProviderResult<AccountProof> {
            Ok(AccountProof::new(Address::random()))
        }

        fn multiproof(
            &self,
            _input: TrieInput,
            _targets: MultiProofTargets,
        ) -> ProviderResult<MultiProof> {
            Ok(MultiProof::default())
        }

        fn witness(
            &self,
            _input: TrieInput,
            _target: HashedPostState,
        ) -> ProviderResult<Vec<Bytes>> {
            Ok(Vec::default())
        }
    }

    #[test]
    fn test_in_memory_state_impl_state_by_hash() {
        let mut state_by_hash = HashMap::default();
        let number = rand::rng().random::<u64>();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let state = Arc::new(create_mock_state(&mut test_block_builder, number, B256::random()));
        state_by_hash.insert(state.hash(), state.clone());

        let in_memory_state = InMemoryState::new(state_by_hash, BTreeMap::new(), None);

        assert_eq!(in_memory_state.state_by_hash(state.hash()), Some(state));
        assert_eq!(in_memory_state.state_by_hash(B256::random()), None);
    }

    #[test]
    fn test_in_memory_state_impl_state_by_number() {
        let mut state_by_hash = HashMap::default();
        let mut hash_by_number = BTreeMap::new();

        let number = rand::rng().random::<u64>();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let state = Arc::new(create_mock_state(&mut test_block_builder, number, B256::random()));
        let hash = state.hash();

        state_by_hash.insert(hash, state.clone());
        hash_by_number.insert(number, hash);

        let in_memory_state = InMemoryState::new(state_by_hash, hash_by_number, None);

        assert_eq!(in_memory_state.state_by_number(number), Some(state));
        assert_eq!(in_memory_state.state_by_number(number + 1), None);
    }

    #[test]
    fn test_in_memory_state_impl_head_state() {
        let mut state_by_hash = HashMap::default();
        let mut hash_by_number = BTreeMap::new();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let state1 = Arc::new(create_mock_state(&mut test_block_builder, 1, B256::random()));
        let hash1 = state1.hash();
        let state2 = Arc::new(create_mock_state(&mut test_block_builder, 2, hash1));
        let hash2 = state2.hash();
        hash_by_number.insert(1, hash1);
        hash_by_number.insert(2, hash2);
        state_by_hash.insert(hash1, state1);
        state_by_hash.insert(hash2, state2);

        let in_memory_state = InMemoryState::new(state_by_hash, hash_by_number, None);
        let head_state = in_memory_state.head_state().unwrap();

        assert_eq!(head_state.hash(), hash2);
        assert_eq!(head_state.number(), 2);
    }

    #[test]
    fn test_in_memory_state_impl_pending_state() {
        let pending_number = rand::rng().random::<u64>();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let pending_state =
            create_mock_state(&mut test_block_builder, pending_number, B256::random());
        let pending_hash = pending_state.hash();

        let in_memory_state =
            InMemoryState::new(HashMap::default(), BTreeMap::new(), Some(pending_state));

        let result = in_memory_state.pending_state();
        assert!(result.is_some());
        let actual_pending_state = result.unwrap();
        assert_eq!(actual_pending_state.block.recovered_block().hash(), pending_hash);
        assert_eq!(actual_pending_state.block.recovered_block().number, pending_number);
    }

    #[test]
    fn test_in_memory_state_impl_no_pending_state() {
        let in_memory_state: InMemoryState =
            InMemoryState::new(HashMap::default(), BTreeMap::new(), None);

        assert_eq!(in_memory_state.pending_state(), None);
    }

    #[test]
    fn test_state() {
        let number = rand::rng().random::<u64>();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block = test_block_builder.get_executed_block_with_number(number, B256::random());

        let state = BlockState::new(block.clone());

        assert_eq!(state.block(), block);
        assert_eq!(state.hash(), block.recovered_block().hash());
        assert_eq!(state.number(), number);
        assert_eq!(state.state_root(), block.recovered_block().state_root);
    }

    #[test]
    fn test_state_receipts() {
        let receipts = vec![vec![Receipt::default()]];
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block =
            test_block_builder.get_executed_block_with_receipts(receipts.clone(), B256::random());

        let state = BlockState::new(block);

        assert_eq!(state.receipts(), &receipts);
    }

    #[test]
    fn test_in_memory_state_chain_update() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block1 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let block2 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let chain = NewCanonicalChain::Commit { new: vec![block1.clone()] };
        state.update_chain(chain);
        assert_eq!(
            state.head_state().unwrap().block_ref().recovered_block().hash(),
            block1.recovered_block().hash()
        );
        assert_eq!(
            state.state_by_number(0).unwrap().block_ref().recovered_block().hash(),
            block1.recovered_block().hash()
        );

        let chain = NewCanonicalChain::Reorg { new: vec![block2.clone()], old: vec![block1.block] };
        state.update_chain(chain);
        assert_eq!(
            state.head_state().unwrap().block_ref().recovered_block().hash(),
            block2.recovered_block().hash()
        );
        assert_eq!(
            state.state_by_number(0).unwrap().block_ref().recovered_block().hash(),
            block2.recovered_block().hash()
        );

        assert_eq!(state.inner.in_memory_state.block_count(), 1);
    }

    #[test]
    fn test_in_memory_state_set_pending_block() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();

        // First random block
        let block1 = test_block_builder.get_executed_block_with_number(0, B256::random());

        // Second block with parent hash of the first block
        let block2 =
            test_block_builder.get_executed_block_with_number(1, block1.recovered_block().hash());

        // Commit the two blocks
        let chain = NewCanonicalChain::Commit { new: vec![block1.clone(), block2.clone()] };
        state.update_chain(chain);

        // Assert that the pending state is None before setting it
        assert!(state.pending_state().is_none());

        // Set the pending block
        state.set_pending_block(block2.clone());

        // Check the pending state
        assert_eq!(
            state.pending_state().unwrap(),
            BlockState::with_parent(block2.clone(), Some(Arc::new(BlockState::new(block1))))
        );

        // Check the pending block
        assert_eq!(state.pending_block().unwrap(), block2.recovered_block().sealed_block().clone());

        // Check the pending block number and hash
        assert_eq!(
            state.pending_block_num_hash().unwrap(),
            BlockNumHash { number: 1, hash: block2.recovered_block().hash() }
        );

        // Check the pending header
        assert_eq!(state.pending_header().unwrap(), block2.recovered_block().header().clone());

        // Check the pending sealed header
        assert_eq!(
            state.pending_sealed_header().unwrap(),
            block2.recovered_block().clone_sealed_header()
        );

        // Check the pending block with senders
        assert_eq!(state.pending_recovered_block().unwrap(), block2.recovered_block().clone());

        // Check the pending block and receipts
        assert_eq!(
            state.pending_block_and_receipts().unwrap(),
            (block2.recovered_block().clone(), vec![])
        );
    }

    #[test]
    fn test_canonical_in_memory_state_state_provider() {
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block1 = test_block_builder.get_executed_block_with_number(1, B256::random());
        let block2 =
            test_block_builder.get_executed_block_with_number(2, block1.recovered_block().hash());
        let block3 =
            test_block_builder.get_executed_block_with_number(3, block2.recovered_block().hash());

        let state1 = Arc::new(BlockState::new(block1.clone()));
        let state2 = Arc::new(BlockState::with_parent(block2.clone(), Some(state1.clone())));
        let state3 = Arc::new(BlockState::with_parent(block3.clone(), Some(state2.clone())));

        let mut blocks = HashMap::default();
        blocks.insert(block1.recovered_block().hash(), state1);
        blocks.insert(block2.recovered_block().hash(), state2);
        blocks.insert(block3.recovered_block().hash(), state3);

        let mut numbers = BTreeMap::new();
        numbers.insert(1, block1.recovered_block().hash());
        numbers.insert(2, block2.recovered_block().hash());
        numbers.insert(3, block3.recovered_block().hash());

        let canonical_state = CanonicalInMemoryState::new(blocks, numbers, None, None, None);

        let historical: StateProviderBox = Box::new(MockStateProvider);

        let overlay_provider =
            canonical_state.state_provider(block3.recovered_block().hash(), historical);

        assert_eq!(overlay_provider.in_memory.len(), 3);
        assert_eq!(overlay_provider.in_memory[0].recovered_block().number, 3);
        assert_eq!(overlay_provider.in_memory[1].recovered_block().number, 2);
        assert_eq!(overlay_provider.in_memory[2].recovered_block().number, 1);

        assert_eq!(
            overlay_provider.in_memory[0].recovered_block().parent_hash,
            overlay_provider.in_memory[1].recovered_block().hash()
        );
        assert_eq!(
            overlay_provider.in_memory[1].recovered_block().parent_hash,
            overlay_provider.in_memory[2].recovered_block().hash()
        );

        let unknown_hash = B256::random();
        let empty_overlay_provider =
            canonical_state.state_provider(unknown_hash, Box::new(MockStateProvider));
        assert_eq!(empty_overlay_provider.in_memory.len(), 0);
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_empty() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let chain: Vec<_> = state.canonical_chain().collect();
        assert!(chain.is_empty());
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_single_block() {
        let block = TestBlockBuilder::eth().get_executed_block_with_number(1, B256::random());
        let hash = block.recovered_block().hash();
        let mut blocks = HashMap::default();
        blocks.insert(hash, Arc::new(BlockState::new(block)));
        let mut numbers = BTreeMap::new();
        numbers.insert(1, hash);

        let state = CanonicalInMemoryState::new(blocks, numbers, None, None, None);
        let chain: Vec<_> = state.canonical_chain().collect();

        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].number(), 1);
        assert_eq!(chain[0].hash(), hash);
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_multiple_blocks() {
        let mut parent_hash = B256::random();
        let mut block_builder = TestBlockBuilder::eth();
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();

        for i in 1..=3 {
            let block = block_builder.get_executed_block_with_number(i, parent_hash);
            let hash = block.recovered_block().hash();
            state.update_blocks(Some(block), None);
            parent_hash = hash;
        }

        let chain: Vec<_> = state.canonical_chain().collect();

        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].number(), 3);
        assert_eq!(chain[1].number(), 2);
        assert_eq!(chain[2].number(), 1);
    }

    // ensures the pending block is not part of the canonical chain
    #[test]
    fn test_canonical_in_memory_state_canonical_chain_with_pending_block() {
        let mut parent_hash = B256::random();
        let mut block_builder = TestBlockBuilder::<EthPrimitives>::eth();
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();

        for i in 1..=2 {
            let block = block_builder.get_executed_block_with_number(i, parent_hash);
            let hash = block.recovered_block().hash();
            state.update_blocks(Some(block), None);
            parent_hash = hash;
        }

        let pending_block = block_builder.get_executed_block_with_number(3, parent_hash);
        state.set_pending_block(pending_block);
        let chain: Vec<_> = state.canonical_chain().collect();

        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].number(), 2);
        assert_eq!(chain[1].number(), 1);
    }

    #[test]
    fn test_block_state_parent_blocks() {
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let chain = create_mock_state_chain(&mut test_block_builder, 4);

        let parents = chain[3].parent_state_chain();
        assert_eq!(parents.len(), 3);
        assert_eq!(parents[0].block().recovered_block().number, 3);
        assert_eq!(parents[1].block().recovered_block().number, 2);
        assert_eq!(parents[2].block().recovered_block().number, 1);

        let parents = chain[2].parent_state_chain();
        assert_eq!(parents.len(), 2);
        assert_eq!(parents[0].block().recovered_block().number, 2);
        assert_eq!(parents[1].block().recovered_block().number, 1);

        let parents = chain[0].parent_state_chain();
        assert_eq!(parents.len(), 0);
    }

    #[test]
    fn test_block_state_single_block_state_chain() {
        let single_block_number = 1;
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let single_block =
            create_mock_state(&mut test_block_builder, single_block_number, B256::random());
        let single_block_hash = single_block.block().recovered_block().hash();

        let parents = single_block.parent_state_chain();
        assert_eq!(parents.len(), 0);

        let block_state_chain = single_block.chain().collect::<Vec<_>>();
        assert_eq!(block_state_chain.len(), 1);
        assert_eq!(block_state_chain[0].block().recovered_block().number, single_block_number);
        assert_eq!(block_state_chain[0].block().recovered_block().hash(), single_block_hash);
    }

    #[test]
    fn test_block_state_chain() {
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let chain = create_mock_state_chain(&mut test_block_builder, 3);

        let block_state_chain = chain[2].chain().collect::<Vec<_>>();
        assert_eq!(block_state_chain.len(), 3);
        assert_eq!(block_state_chain[0].block().recovered_block().number, 3);
        assert_eq!(block_state_chain[1].block().recovered_block().number, 2);
        assert_eq!(block_state_chain[2].block().recovered_block().number, 1);

        let block_state_chain = chain[1].chain().collect::<Vec<_>>();
        assert_eq!(block_state_chain.len(), 2);
        assert_eq!(block_state_chain[0].block().recovered_block().number, 2);
        assert_eq!(block_state_chain[1].block().recovered_block().number, 1);

        let block_state_chain = chain[0].chain().collect::<Vec<_>>();
        assert_eq!(block_state_chain.len(), 1);
        assert_eq!(block_state_chain[0].block().recovered_block().number, 1);
    }

    #[test]
    fn test_to_chain_notification() {
        // Generate 4 blocks
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block0 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let block1 =
            test_block_builder.get_executed_block_with_number(1, block0.recovered_block.hash());
        let block1a =
            test_block_builder.get_executed_block_with_number(1, block0.recovered_block.hash());
        let block2 =
            test_block_builder.get_executed_block_with_number(2, block1.recovered_block.hash());
        let block2a =
            test_block_builder.get_executed_block_with_number(2, block1.recovered_block.hash());

        let sample_execution_outcome = ExecutionOutcome {
            receipts: vec![vec![], vec![]],
            requests: vec![Requests::default(), Requests::default()],
            ..Default::default()
        };

        // Test commit notification
        let chain_commit = NewCanonicalChain::Commit { new: vec![block0.clone(), block1.clone()] };

        assert_eq!(
            chain_commit.to_chain_notification(),
            CanonStateNotification::Commit {
                new: Arc::new(Chain::new(
                    vec![block0.recovered_block().clone(), block1.recovered_block().clone()],
                    sample_execution_outcome.clone(),
                    None
                ))
            }
        );

        // Test reorg notification
        let chain_reorg = NewCanonicalChain::Reorg {
            new: vec![block1a.clone(), block2a.clone()],
            old: vec![block1.block.clone(), block2.block.clone()],
        };

        assert_eq!(
            chain_reorg.to_chain_notification(),
            CanonStateNotification::Reorg {
                old: Arc::new(Chain::new(
                    vec![block1.recovered_block().clone(), block2.recovered_block().clone()],
                    sample_execution_outcome.clone(),
                    None
                )),
                new: Arc::new(Chain::new(
                    vec![block1a.recovered_block().clone(), block2a.recovered_block().clone()],
                    sample_execution_outcome,
                    None
                ))
            }
        );
    }
}
