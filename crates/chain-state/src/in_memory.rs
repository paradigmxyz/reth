//! Types for tracking the canonical chain state in memory.

use crate::{
    CanonStateNotification, CanonStateNotificationSender, CanonStateNotifications,
    ChainInfoTracker, MemoryOverlayStateProvider,
};
use alloy_eips::BlockNumHash;
use alloy_primitives::{map::HashMap, Address, TxHash, B256};
use parking_lot::RwLock;
use reth_chainspec::ChainInfo;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_metrics::{metrics::Gauge, Metrics};
use reth_primitives::{
    BlockWithSenders, Header, Receipt, Receipts, SealedBlock, SealedBlockWithSenders, SealedHeader,
    TransactionMeta, TransactionSigned,
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
pub(crate) struct InMemoryState {
    /// All canonical blocks that are not on disk yet.
    blocks: RwLock<HashMap<B256, Arc<BlockState>>>,
    /// Mapping of block numbers to block hashes.
    numbers: RwLock<BTreeMap<u64, B256>>,
    /// The pending block that has not yet been made canonical.
    pending: watch::Sender<Option<BlockState>>,
    /// Metrics for the in-memory state.
    metrics: InMemoryStateMetrics,
}

impl InMemoryState {
    pub(crate) fn new(
        blocks: HashMap<B256, Arc<BlockState>>,
        numbers: BTreeMap<u64, B256>,
        pending: Option<BlockState>,
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
    pub(crate) fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState>> {
        self.blocks.read().get(&hash).cloned()
    }

    /// Returns the state for a given block number.
    pub(crate) fn state_by_number(&self, number: u64) -> Option<Arc<BlockState>> {
        let hash = self.hash_by_number(number)?;
        self.state_by_hash(hash)
    }

    /// Returns the hash for a specific block number
    pub(crate) fn hash_by_number(&self, number: u64) -> Option<B256> {
        self.numbers.read().get(&number).copied()
    }

    /// Returns the current chain head state.
    pub(crate) fn head_state(&self) -> Option<Arc<BlockState>> {
        let hash = *self.numbers.read().last_key_value()?.1;
        self.state_by_hash(hash)
    }

    /// Returns the pending state corresponding to the current head plus one,
    /// from the payload received in newPayload that does not have a FCU yet.
    pub(crate) fn pending_state(&self) -> Option<BlockState> {
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
pub(crate) struct CanonicalInMemoryStateInner {
    /// Tracks certain chain information, such as the canonical head, safe head, and finalized
    /// head.
    pub(crate) chain_info_tracker: ChainInfoTracker,
    /// Tracks blocks at the tip of the chain that have not been persisted to disk yet.
    pub(crate) in_memory_state: InMemoryState,
    /// A broadcast stream that emits events when the canonical chain is updated.
    pub(crate) canon_state_notification_sender: CanonStateNotificationSender,
}

impl CanonicalInMemoryStateInner {
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

/// This type is responsible for providing the blocks, receipts, and state for
/// all canonical blocks not on disk yet and keeps track of the block range that
/// is in memory.
#[derive(Debug, Clone)]
pub struct CanonicalInMemoryState {
    pub(crate) inner: Arc<CanonicalInMemoryStateInner>,
}

impl CanonicalInMemoryState {
    /// Create a new in-memory state with the given blocks, numbers, pending state, and optional
    /// finalized header.
    pub fn new(
        blocks: HashMap<B256, Arc<BlockState>>,
        numbers: BTreeMap<u64, B256>,
        pending: Option<BlockState>,
        finalized: Option<SealedHeader>,
        safe: Option<SealedHeader>,
    ) -> Self {
        let in_memory_state = InMemoryState::new(blocks, numbers, pending);
        let header = in_memory_state
            .head_state()
            .map_or_else(SealedHeader::default, |state| state.block_ref().block().header.clone());
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
        head: SealedHeader,
        finalized: Option<SealedHeader>,
        safe: Option<SealedHeader>,
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
    pub fn header_by_hash(&self, hash: B256) -> Option<SealedHeader> {
        self.state_by_hash(hash).map(|block| block.block_ref().block.header.clone())
    }

    /// Clears all entries in the in memory state.
    pub fn clear_state(&self) {
        self.inner.clear()
    }

    /// Updates the pending block with the given block.
    ///
    /// Note: This assumes that the parent block of the pending block is canonical.
    pub fn set_pending_block(&self, pending: ExecutedBlock) {
        // fetch the state of the pending block's parent block
        let parent = self.state_by_hash(pending.block().parent_hash);
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
    fn update_blocks<I>(&self, new_blocks: I, reorged: I)
    where
        I: IntoIterator<Item = ExecutedBlock>,
    {
        {
            // acquire locks, starting with the numbers lock
            let mut numbers = self.inner.in_memory_state.numbers.write();
            let mut blocks = self.inner.in_memory_state.blocks.write();

            // we first remove the blocks from the reorged chain
            for block in reorged {
                let hash = block.block().hash();
                let number = block.block().number;
                blocks.remove(&hash);
                numbers.remove(&number);
            }

            // insert the new blocks
            for block in new_blocks {
                let parent = blocks.get(&block.block().parent_hash).cloned();
                let block_state = BlockState::with_parent(block.clone(), parent);
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
    pub fn update_chain(&self, new_chain: NewCanonicalChain) {
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
                .filter(|(_, b)| b.block_ref().block().number > persisted_height)
                .map(|(_, b)| b.block.clone())
                .collect::<Vec<_>>();

            // sort the blocks by number so we can insert them back in natural order (low -> high)
            old_blocks.sort_unstable_by_key(|block| block.block().number);

            // re-insert the blocks in natural order and connect them to their parent blocks
            for block in old_blocks {
                let parent = blocks.get(&block.block().parent_hash).cloned();
                let block_state = BlockState::with_parent(block.clone(), parent);
                let hash = block_state.hash();
                let number = block_state.number();

                // append new blocks
                blocks.insert(hash, Arc::new(block_state));
                numbers.insert(number, hash);
            }

            // also shift the pending state if it exists
            self.inner.in_memory_state.pending.send_modify(|p| {
                if let Some(p) = p.as_mut() {
                    p.parent = blocks.get(&p.block_ref().block.parent_hash).cloned();
                }
            });
        }
        self.inner.in_memory_state.update_metrics();
    }

    /// Returns in memory state corresponding the given hash.
    pub fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState>> {
        self.inner.in_memory_state.state_by_hash(hash)
    }

    /// Returns in memory state corresponding the block number.
    pub fn state_by_number(&self, number: u64) -> Option<Arc<BlockState>> {
        self.inner.in_memory_state.state_by_number(number)
    }

    /// Returns the in memory head state.
    pub fn head_state(&self) -> Option<Arc<BlockState>> {
        self.inner.in_memory_state.head_state()
    }

    /// Returns the in memory pending state.
    pub fn pending_state(&self) -> Option<BlockState> {
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

    /// Hook for transition configuration exchanged.
    pub fn on_transition_configuration_exchanged(&self) {
        self.inner.chain_info_tracker.on_transition_configuration_exchanged();
    }

    /// Returns the timestamp of the last transition configuration exchanged,
    pub fn last_exchanged_transition_configuration_timestamp(&self) -> Option<Instant> {
        self.inner.chain_info_tracker.last_transition_configuration_exchanged_at()
    }

    /// Canonical head setter.
    pub fn set_canonical_head(&self, header: SealedHeader) {
        self.inner.chain_info_tracker.set_canonical_head(header);
    }

    /// Safe head setter.
    pub fn set_safe(&self, header: SealedHeader) {
        self.inner.chain_info_tracker.set_safe(header);
    }

    /// Finalized head setter.
    pub fn set_finalized(&self, header: SealedHeader) {
        self.inner.chain_info_tracker.set_finalized(header);
    }

    /// Canonical head getter.
    pub fn get_canonical_head(&self) -> SealedHeader {
        self.inner.chain_info_tracker.get_canonical_head()
    }

    /// Finalized header getter.
    pub fn get_finalized_header(&self) -> Option<SealedHeader> {
        self.inner.chain_info_tracker.get_finalized_header()
    }

    /// Safe header getter.
    pub fn get_safe_header(&self) -> Option<SealedHeader> {
        self.inner.chain_info_tracker.get_safe_header()
    }

    /// Returns the `SealedHeader` corresponding to the pending state.
    pub fn pending_sealed_header(&self) -> Option<SealedHeader> {
        self.pending_state().map(|h| h.block_ref().block().header.clone())
    }

    /// Returns the `Header` corresponding to the pending state.
    pub fn pending_header(&self) -> Option<Header> {
        self.pending_sealed_header().map(|sealed_header| sealed_header.unseal())
    }

    /// Returns the `SealedBlock` corresponding to the pending state.
    pub fn pending_block(&self) -> Option<SealedBlock> {
        self.pending_state().map(|block_state| block_state.block_ref().block().clone())
    }

    /// Returns the `SealedBlockWithSenders` corresponding to the pending state.
    pub fn pending_block_with_senders(&self) -> Option<SealedBlockWithSenders> {
        self.pending_state()
            .and_then(|block_state| block_state.block_ref().block().clone().seal_with_senders())
    }

    /// Returns a tuple with the `SealedBlock` corresponding to the pending
    /// state and a vector of its `Receipt`s.
    pub fn pending_block_and_receipts(&self) -> Option<(SealedBlock, Vec<Receipt>)> {
        self.pending_state().map(|block_state| {
            (block_state.block_ref().block().clone(), block_state.executed_block_receipts())
        })
    }

    /// Subscribe to new blocks events.
    pub fn subscribe_canon_state(&self) -> CanonStateNotifications {
        self.inner.canon_state_notification_sender.subscribe()
    }

    /// Subscribe to new safe block events.
    pub fn subscribe_safe_block(&self) -> watch::Receiver<Option<SealedHeader>> {
        self.inner.chain_info_tracker.subscribe_safe_block()
    }

    /// Subscribe to new finalized block events.
    pub fn subscribe_finalized_block(&self) -> watch::Receiver<Option<SealedHeader>> {
        self.inner.chain_info_tracker.subscribe_finalized_block()
    }

    /// Attempts to send a new [`CanonStateNotification`] to all active Receiver handles.
    pub fn notify_canon_state(&self, event: CanonStateNotification) {
        self.inner.canon_state_notification_sender.send(event).ok();
    }

    /// Return state provider with reference to in-memory blocks that overlay database state.
    ///
    /// This merges the state of all blocks that are part of the chain that the requested block is
    /// the head of. This includes all blocks that connect back to the canonical block on disk.
    pub fn state_provider_from_state(
        &self,
        state: &BlockState,
        historical: StateProviderBox,
    ) -> MemoryOverlayStateProvider {
        let in_memory = state.chain().into_iter().map(|block_state| block_state.block()).collect();

        MemoryOverlayStateProvider::new(historical, in_memory)
    }

    /// Return state provider with reference to in-memory blocks that overlay database state.
    ///
    /// This merges the state of all blocks that are part of the chain that the requested block is
    /// the head of. This includes all blocks that connect back to the canonical block on disk.
    pub fn state_provider(
        &self,
        hash: B256,
        historical: StateProviderBox,
    ) -> MemoryOverlayStateProvider {
        let in_memory = if let Some(state) = self.state_by_hash(hash) {
            state.chain().into_iter().map(|block_state| block_state.block()).collect()
        } else {
            Vec::new()
        };

        MemoryOverlayStateProvider::new(historical, in_memory)
    }

    /// Returns an iterator over all __canonical blocks__ in the in-memory state, from newest to
    /// oldest (highest to lowest).
    ///
    /// This iterator contains a snapshot of the in-memory state at the time of the call.
    pub fn canonical_chain(&self) -> impl Iterator<Item = Arc<BlockState>> {
        self.inner.in_memory_state.head_state().into_iter().flat_map(|head| head.iter())
    }

    /// Returns a `TransactionSigned` for the given `TxHash` if found.
    pub fn transaction_by_hash(&self, hash: TxHash) -> Option<TransactionSigned> {
        for block_state in self.canonical_chain() {
            if let Some(tx) =
                block_state.block_ref().block().body.transactions().find(|tx| tx.hash() == hash)
            {
                return Some(tx.clone())
            }
        }
        None
    }

    /// Returns a tuple with `TransactionSigned` and `TransactionMeta` for the
    /// given `TxHash` if found.
    pub fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> Option<(TransactionSigned, TransactionMeta)> {
        for block_state in self.canonical_chain() {
            if let Some((index, tx)) = block_state
                .block_ref()
                .block()
                .body
                .transactions()
                .enumerate()
                .find(|(_, tx)| tx.hash() == tx_hash)
            {
                let meta = TransactionMeta {
                    tx_hash,
                    index: index as u64,
                    block_hash: block_state.hash(),
                    block_number: block_state.block_ref().block.number,
                    base_fee: block_state.block_ref().block.header.base_fee_per_gas,
                    timestamp: block_state.block_ref().block.timestamp,
                    excess_blob_gas: block_state.block_ref().block.excess_blob_gas,
                };
                return Some((tx.clone(), meta))
            }
        }
        None
    }
}

/// State after applying the given block, this block is part of the canonical chain that partially
/// stored in memory and can be traced back to a canonical block on disk.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BlockState {
    /// The executed block that determines the state after this block has been executed.
    block: ExecutedBlock,
    /// The block's parent block if it exists.
    parent: Option<Arc<BlockState>>,
}

#[allow(dead_code)]
impl BlockState {
    /// [`BlockState`] constructor.
    pub const fn new(block: ExecutedBlock) -> Self {
        Self { block, parent: None }
    }

    /// [`BlockState`] constructor with parent.
    pub const fn with_parent(block: ExecutedBlock, parent: Option<Arc<Self>>) -> Self {
        Self { block, parent }
    }

    /// Returns the hash and block of the on disk block this state can be traced back to.
    pub fn anchor(&self) -> BlockNumHash {
        if let Some(parent) = &self.parent {
            parent.anchor()
        } else {
            self.block.block().parent_num_hash()
        }
    }

    /// Returns the executed block that determines the state.
    pub fn block(&self) -> ExecutedBlock {
        self.block.clone()
    }

    /// Returns a reference to the executed block that determines the state.
    pub const fn block_ref(&self) -> &ExecutedBlock {
        &self.block
    }

    /// Returns the block with senders for the state.
    pub fn block_with_senders(&self) -> BlockWithSenders {
        let block = self.block.block().clone();
        let senders = self.block.senders().clone();
        BlockWithSenders { block: block.unseal(), senders }
    }

    /// Returns the sealed block with senders for the state.
    pub fn sealed_block_with_senders(&self) -> SealedBlockWithSenders {
        let block = self.block.block().clone();
        let senders = self.block.senders().clone();
        SealedBlockWithSenders { block, senders }
    }

    /// Returns the hash of executed block that determines the state.
    pub fn hash(&self) -> B256 {
        self.block.block().hash()
    }

    /// Returns the block number of executed block that determines the state.
    pub fn number(&self) -> u64 {
        self.block.block().number
    }

    /// Returns the state root after applying the executed block that determines
    /// the state.
    pub fn state_root(&self) -> B256 {
        self.block.block().header.state_root
    }

    /// Returns the `Receipts` of executed block that determines the state.
    pub fn receipts(&self) -> &Receipts {
        &self.block.execution_outcome().receipts
    }

    /// Returns a vector of `Receipt` of executed block that determines the state.
    /// We assume that the `Receipts` in the executed block `ExecutionOutcome`
    /// has only one element corresponding to the executed block associated to
    /// the state.
    pub fn executed_block_receipts(&self) -> Vec<Receipt> {
        let receipts = self.receipts();

        debug_assert!(
            receipts.receipt_vec.len() <= 1,
            "Expected at most one block's worth of receipts, found {}",
            receipts.receipt_vec.len()
        );

        receipts
            .receipt_vec
            .first()
            .map(|block_receipts| {
                block_receipts.iter().filter_map(|opt_receipt| opt_receipt.clone()).collect()
            })
            .unwrap_or_default()
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
    pub fn chain(&self) -> Vec<&Self> {
        let mut chain = vec![self];
        self.append_parent_chain(&mut chain);
        chain
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
}

/// Represents an executed block stored in-memory.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ExecutedBlock {
    /// Sealed block the rest of fields refer to.
    pub block: Arc<SealedBlock>,
    /// Block's senders.
    pub senders: Arc<Vec<Address>>,
    /// Block's execution outcome.
    pub execution_output: Arc<ExecutionOutcome>,
    /// Block's hashedst state.
    pub hashed_state: Arc<HashedPostState>,
    /// Trie updates that result of applying the block.
    pub trie: Arc<TrieUpdates>,
}

impl ExecutedBlock {
    /// [`ExecutedBlock`] constructor.
    pub const fn new(
        block: Arc<SealedBlock>,
        senders: Arc<Vec<Address>>,
        execution_output: Arc<ExecutionOutcome>,
        hashed_state: Arc<HashedPostState>,
        trie: Arc<TrieUpdates>,
    ) -> Self {
        Self { block, senders, execution_output, hashed_state, trie }
    }

    /// Returns a reference to the executed block.
    pub fn block(&self) -> &SealedBlock {
        &self.block
    }

    /// Returns a reference to the block's senders
    pub fn senders(&self) -> &Vec<Address> {
        &self.senders
    }

    /// Returns a [`SealedBlockWithSenders`]
    ///
    /// Note: this clones the block and senders.
    pub fn sealed_block_with_senders(&self) -> SealedBlockWithSenders {
        SealedBlockWithSenders { block: (*self.block).clone(), senders: (*self.senders).clone() }
    }

    /// Returns a reference to the block's execution outcome
    pub fn execution_outcome(&self) -> &ExecutionOutcome {
        &self.execution_output
    }

    /// Returns a reference to the hashed state result of the execution outcome
    pub fn hashed_state(&self) -> &HashedPostState {
        &self.hashed_state
    }

    /// Returns a reference to the trie updates for the block
    pub fn trie_updates(&self) -> &TrieUpdates {
        &self.trie
    }
}

/// Non-empty chain of blocks.
#[derive(Debug)]
pub enum NewCanonicalChain {
    /// A simple append to the current canonical head
    Commit {
        /// all blocks that lead back to the canonical head
        new: Vec<ExecutedBlock>,
    },
    /// A reorged chain consists of two chains that trace back to a shared ancestor block at which
    /// point they diverge.
    Reorg {
        /// All blocks of the _new_ chain
        new: Vec<ExecutedBlock>,
        /// All blocks of the _old_ chain
        old: Vec<ExecutedBlock>,
    },
}

impl NewCanonicalChain {
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
    pub fn to_chain_notification(&self) -> CanonStateNotification {
        match self {
            Self::Commit { new } => {
                let new = Arc::new(new.iter().fold(Chain::default(), |mut chain, exec| {
                    chain.append_block(
                        exec.sealed_block_with_senders(),
                        exec.execution_outcome().clone(),
                    );
                    chain
                }));
                CanonStateNotification::Commit { new }
            }
            Self::Reorg { new, old } => {
                let new = Arc::new(new.iter().fold(Chain::default(), |mut chain, exec| {
                    chain.append_block(
                        exec.sealed_block_with_senders(),
                        exec.execution_outcome().clone(),
                    );
                    chain
                }));
                let old = Arc::new(old.iter().fold(Chain::default(), |mut chain, exec| {
                    chain.append_block(
                        exec.sealed_block_with_senders(),
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
    pub fn tip(&self) -> &SealedBlock {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => {
                new.last().expect("non empty blocks").block()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestBlockBuilder;
    use alloy_primitives::{map::HashSet, BlockNumber, Bytes, StorageKey, StorageValue};
    use rand::Rng;
    use reth_errors::ProviderResult;
    use reth_primitives::{Account, Bytecode, Receipt, Requests};
    use reth_storage_api::{
        AccountReader, BlockHashReader, StateProofProvider, StateProvider, StateRootProvider,
        StorageRootProvider,
    };
    use reth_trie::{AccountProof, HashedStorage, MultiProof, StorageProof, TrieInput};

    fn create_mock_state(
        test_block_builder: &mut TestBlockBuilder,
        block_number: u64,
        parent_hash: B256,
    ) -> BlockState {
        BlockState::new(
            test_block_builder.get_executed_block_with_number(block_number, parent_hash),
        )
    }

    fn create_mock_state_chain(
        test_block_builder: &mut TestBlockBuilder,
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

        fn bytecode_by_hash(&self, _code_hash: B256) -> ProviderResult<Option<Bytecode>> {
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
        fn basic_account(&self, _address: Address) -> ProviderResult<Option<Account>> {
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
            _targets: HashMap<B256, HashSet<B256>>,
        ) -> ProviderResult<MultiProof> {
            Ok(MultiProof::default())
        }

        fn witness(
            &self,
            _input: TrieInput,
            _target: HashedPostState,
        ) -> ProviderResult<HashMap<B256, Bytes>> {
            Ok(HashMap::default())
        }
    }

    #[test]
    fn test_in_memory_state_impl_state_by_hash() {
        let mut state_by_hash = HashMap::default();
        let number = rand::thread_rng().gen::<u64>();
        let mut test_block_builder = TestBlockBuilder::default();
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

        let number = rand::thread_rng().gen::<u64>();
        let mut test_block_builder = TestBlockBuilder::default();
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
        let mut test_block_builder = TestBlockBuilder::default();
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
        let pending_number = rand::thread_rng().gen::<u64>();
        let mut test_block_builder = TestBlockBuilder::default();
        let pending_state =
            create_mock_state(&mut test_block_builder, pending_number, B256::random());
        let pending_hash = pending_state.hash();

        let in_memory_state =
            InMemoryState::new(HashMap::default(), BTreeMap::new(), Some(pending_state));

        let result = in_memory_state.pending_state();
        assert!(result.is_some());
        let actual_pending_state = result.unwrap();
        assert_eq!(actual_pending_state.block.block().hash(), pending_hash);
        assert_eq!(actual_pending_state.block.block().number, pending_number);
    }

    #[test]
    fn test_in_memory_state_impl_no_pending_state() {
        let in_memory_state = InMemoryState::new(HashMap::default(), BTreeMap::new(), None);

        assert_eq!(in_memory_state.pending_state(), None);
    }

    #[test]
    fn test_state_new() {
        let number = rand::thread_rng().gen::<u64>();
        let mut test_block_builder = TestBlockBuilder::default();
        let block = test_block_builder.get_executed_block_with_number(number, B256::random());

        let state = BlockState::new(block.clone());

        assert_eq!(state.block(), block);
    }

    #[test]
    fn test_state_block() {
        let number = rand::thread_rng().gen::<u64>();
        let mut test_block_builder = TestBlockBuilder::default();
        let block = test_block_builder.get_executed_block_with_number(number, B256::random());

        let state = BlockState::new(block.clone());

        assert_eq!(state.block(), block);
    }

    #[test]
    fn test_state_hash() {
        let number = rand::thread_rng().gen::<u64>();
        let mut test_block_builder = TestBlockBuilder::default();
        let block = test_block_builder.get_executed_block_with_number(number, B256::random());

        let state = BlockState::new(block.clone());

        assert_eq!(state.hash(), block.block.hash());
    }

    #[test]
    fn test_state_number() {
        let number = rand::thread_rng().gen::<u64>();
        let mut test_block_builder = TestBlockBuilder::default();
        let block = test_block_builder.get_executed_block_with_number(number, B256::random());

        let state = BlockState::new(block);

        assert_eq!(state.number(), number);
    }

    #[test]
    fn test_state_state_root() {
        let number = rand::thread_rng().gen::<u64>();
        let mut test_block_builder = TestBlockBuilder::default();
        let block = test_block_builder.get_executed_block_with_number(number, B256::random());

        let state = BlockState::new(block.clone());

        assert_eq!(state.state_root(), block.block().state_root);
    }

    #[test]
    fn test_state_receipts() {
        let receipts = Receipts { receipt_vec: vec![vec![Some(Receipt::default())]] };
        let mut test_block_builder = TestBlockBuilder::default();
        let block =
            test_block_builder.get_executed_block_with_receipts(receipts.clone(), B256::random());

        let state = BlockState::new(block);

        assert_eq!(state.receipts(), &receipts);
    }

    #[test]
    fn test_in_memory_state_chain_update() {
        let state = CanonicalInMemoryState::empty();
        let mut test_block_builder = TestBlockBuilder::default();
        let block1 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let block2 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let chain = NewCanonicalChain::Commit { new: vec![block1.clone()] };
        state.update_chain(chain);
        assert_eq!(state.head_state().unwrap().block_ref().block().hash(), block1.block().hash());
        assert_eq!(
            state.state_by_number(0).unwrap().block_ref().block().hash(),
            block1.block().hash()
        );

        let chain = NewCanonicalChain::Reorg { new: vec![block2.clone()], old: vec![block1] };
        state.update_chain(chain);
        assert_eq!(state.head_state().unwrap().block_ref().block().hash(), block2.block().hash());
        assert_eq!(
            state.state_by_number(0).unwrap().block_ref().block().hash(),
            block2.block().hash()
        );

        assert_eq!(state.inner.in_memory_state.block_count(), 1);
    }

    #[test]
    fn test_in_memory_state_set_pending_block() {
        let state = CanonicalInMemoryState::empty();
        let mut test_block_builder = TestBlockBuilder::default();

        // First random block
        let block1 = test_block_builder.get_executed_block_with_number(0, B256::random());

        // Second block with parent hash of the first block
        let block2 = test_block_builder.get_executed_block_with_number(1, block1.block().hash());

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
        assert_eq!(state.pending_block().unwrap(), block2.block().clone());

        // Check the pending block number and hash
        assert_eq!(
            state.pending_block_num_hash().unwrap(),
            BlockNumHash { number: 1, hash: block2.block().hash() }
        );

        // Check the pending header
        assert_eq!(state.pending_header().unwrap(), block2.block().header.header().clone());

        // Check the pending sealed header
        assert_eq!(state.pending_sealed_header().unwrap(), block2.block().header.clone());

        // Check the pending block with senders
        assert_eq!(
            state.pending_block_with_senders().unwrap(),
            block2.block().clone().seal_with_senders().unwrap()
        );

        // Check the pending block and receipts
        assert_eq!(state.pending_block_and_receipts().unwrap(), (block2.block().clone(), vec![]));
    }

    #[test]
    fn test_canonical_in_memory_state_state_provider() {
        let mut test_block_builder = TestBlockBuilder::default();
        let block1 = test_block_builder.get_executed_block_with_number(1, B256::random());
        let block2 = test_block_builder.get_executed_block_with_number(2, block1.block().hash());
        let block3 = test_block_builder.get_executed_block_with_number(3, block2.block().hash());

        let state1 = Arc::new(BlockState::new(block1.clone()));
        let state2 = Arc::new(BlockState::with_parent(block2.clone(), Some(state1.clone())));
        let state3 = Arc::new(BlockState::with_parent(block3.clone(), Some(state2.clone())));

        let mut blocks = HashMap::default();
        blocks.insert(block1.block().hash(), state1);
        blocks.insert(block2.block().hash(), state2);
        blocks.insert(block3.block().hash(), state3);

        let mut numbers = BTreeMap::new();
        numbers.insert(1, block1.block().hash());
        numbers.insert(2, block2.block().hash());
        numbers.insert(3, block3.block().hash());

        let canonical_state = CanonicalInMemoryState::new(blocks, numbers, None, None, None);

        let historical: StateProviderBox = Box::new(MockStateProvider);

        let overlay_provider = canonical_state.state_provider(block3.block().hash(), historical);

        assert_eq!(overlay_provider.in_memory.len(), 3);
        assert_eq!(overlay_provider.in_memory[0].block().number, 3);
        assert_eq!(overlay_provider.in_memory[1].block().number, 2);
        assert_eq!(overlay_provider.in_memory[2].block().number, 1);

        assert_eq!(
            overlay_provider.in_memory[0].block().parent_hash,
            overlay_provider.in_memory[1].block().hash()
        );
        assert_eq!(
            overlay_provider.in_memory[1].block().parent_hash,
            overlay_provider.in_memory[2].block().hash()
        );

        let unknown_hash = B256::random();
        let empty_overlay_provider =
            canonical_state.state_provider(unknown_hash, Box::new(MockStateProvider));
        assert_eq!(empty_overlay_provider.in_memory.len(), 0);
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_empty() {
        let state = CanonicalInMemoryState::empty();
        let chain: Vec<_> = state.canonical_chain().collect();
        assert!(chain.is_empty());
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_single_block() {
        let block = TestBlockBuilder::default().get_executed_block_with_number(1, B256::random());
        let hash = block.block().hash();
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
        let mut block_builder = TestBlockBuilder::default();
        let state = CanonicalInMemoryState::empty();

        for i in 1..=3 {
            let block = block_builder.get_executed_block_with_number(i, parent_hash);
            let hash = block.block().hash();
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
        let mut block_builder = TestBlockBuilder::default();
        let state = CanonicalInMemoryState::empty();

        for i in 1..=2 {
            let block = block_builder.get_executed_block_with_number(i, parent_hash);
            let hash = block.block().hash();
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
        let mut test_block_builder = TestBlockBuilder::default();
        let chain = create_mock_state_chain(&mut test_block_builder, 4);

        let parents = chain[3].parent_state_chain();
        assert_eq!(parents.len(), 3);
        assert_eq!(parents[0].block().block.number, 3);
        assert_eq!(parents[1].block().block.number, 2);
        assert_eq!(parents[2].block().block.number, 1);

        let parents = chain[2].parent_state_chain();
        assert_eq!(parents.len(), 2);
        assert_eq!(parents[0].block().block.number, 2);
        assert_eq!(parents[1].block().block.number, 1);

        let parents = chain[0].parent_state_chain();
        assert_eq!(parents.len(), 0);
    }

    #[test]
    fn test_block_state_single_block_state_chain() {
        let single_block_number = 1;
        let mut test_block_builder = TestBlockBuilder::default();
        let single_block =
            create_mock_state(&mut test_block_builder, single_block_number, B256::random());
        let single_block_hash = single_block.block().block.hash();

        let parents = single_block.parent_state_chain();
        assert_eq!(parents.len(), 0);

        let block_state_chain = single_block.chain();
        assert_eq!(block_state_chain.len(), 1);
        assert_eq!(block_state_chain[0].block().block.number, single_block_number);
        assert_eq!(block_state_chain[0].block().block.hash(), single_block_hash);
    }

    #[test]
    fn test_block_state_chain() {
        let mut test_block_builder = TestBlockBuilder::default();
        let chain = create_mock_state_chain(&mut test_block_builder, 3);

        let block_state_chain = chain[2].chain();
        assert_eq!(block_state_chain.len(), 3);
        assert_eq!(block_state_chain[0].block().block.number, 3);
        assert_eq!(block_state_chain[1].block().block.number, 2);
        assert_eq!(block_state_chain[2].block().block.number, 1);

        let block_state_chain = chain[1].chain();
        assert_eq!(block_state_chain.len(), 2);
        assert_eq!(block_state_chain[0].block().block.number, 2);
        assert_eq!(block_state_chain[1].block().block.number, 1);

        let block_state_chain = chain[0].chain();
        assert_eq!(block_state_chain.len(), 1);
        assert_eq!(block_state_chain[0].block().block.number, 1);
    }

    #[test]
    fn test_to_chain_notification() {
        // Generate 4 blocks
        let mut test_block_builder = TestBlockBuilder::default();
        let block0 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let block1 = test_block_builder.get_executed_block_with_number(1, block0.block.hash());
        let block1a = test_block_builder.get_executed_block_with_number(1, block0.block.hash());
        let block2 = test_block_builder.get_executed_block_with_number(2, block1.block.hash());
        let block2a = test_block_builder.get_executed_block_with_number(2, block1.block.hash());

        let sample_execution_outcome = ExecutionOutcome {
            receipts: Receipts::from_iter([vec![], vec![]]),
            requests: vec![Requests::default(), Requests::default()],
            ..Default::default()
        };

        // Test commit notification
        let chain_commit = NewCanonicalChain::Commit { new: vec![block0.clone(), block1.clone()] };

        assert_eq!(
            chain_commit.to_chain_notification(),
            CanonStateNotification::Commit {
                new: Arc::new(Chain::new(
                    vec![block0.sealed_block_with_senders(), block1.sealed_block_with_senders()],
                    sample_execution_outcome.clone(),
                    None
                ))
            }
        );

        // Test reorg notification
        let chain_reorg = NewCanonicalChain::Reorg {
            new: vec![block1a.clone(), block2a.clone()],
            old: vec![block1.clone(), block2.clone()],
        };

        assert_eq!(
            chain_reorg.to_chain_notification(),
            CanonStateNotification::Reorg {
                old: Arc::new(Chain::new(
                    vec![block1.sealed_block_with_senders(), block2.sealed_block_with_senders()],
                    sample_execution_outcome.clone(),
                    None
                )),
                new: Arc::new(Chain::new(
                    vec![block1a.sealed_block_with_senders(), block2a.sealed_block_with_senders()],
                    sample_execution_outcome,
                    None
                ))
            }
        );
    }
}
