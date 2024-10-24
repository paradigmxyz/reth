//! Implementation of [`BlockchainTree`]

use crate::{
    metrics::{MakeCanonicalAction, MakeCanonicalDurationsRecorder, TreeMetrics},
    state::{SidechainId, TreeState},
    AppendableChain, BlockIndices, BlockchainTreeConfig, ExecutionData, TreeExternals,
};
use alloy_eips::{BlockNumHash, ForkBlock};
use alloy_primitives::{BlockHash, BlockNumber, B256, U256};
use reth_blockchain_tree_api::{
    error::{BlockchainTreeError, CanonicalError, InsertBlockError, InsertBlockErrorKind},
    BlockAttachment, BlockStatus, BlockValidationKind, CanonicalOutcome, InsertPayloadOk,
};
use reth_consensus::{Consensus, ConsensusError};
use reth_evm::execute::BlockExecutorProvider;
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_node_types::NodeTypesWithDB;
use reth_primitives::{
    EthereumHardfork, GotExpected, Hardforks, Receipt, SealedBlock, SealedBlockWithSenders,
    SealedHeader, StaticFileSegment,
};
use reth_provider::{
    providers::ProviderNodeTypes, BlockExecutionWriter, BlockNumReader, BlockWriter,
    CanonStateNotification, CanonStateNotificationSender, CanonStateNotifications,
    ChainSpecProvider, ChainSplit, ChainSplitTarget, DisplayBlocksChain, HeaderProvider,
    ProviderError, StaticFileProviderFactory,
};
use reth_stages_api::{MetricEvent, MetricEventsSender};
use reth_storage_errors::provider::{ProviderResult, RootMismatch};
use reth_trie::{hashed_cursor::HashedPostStateCursorFactory, StateRoot};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseStateRoot};
use std::{
    collections::{btree_map::Entry, BTreeMap, HashSet},
    sync::Arc,
};
use tracing::{debug, error, info, instrument, trace, warn};

#[cfg_attr(doc, aquamarine::aquamarine)]
/// A Tree of chains.
///
/// The flowchart represents all the states a block can have inside the tree.
///
/// - Green blocks belong to the canonical chain and are saved inside the database.
/// - Pending blocks and sidechains are found in-memory inside [`BlockchainTree`].
///
/// Both pending chains and sidechains have the same mechanisms, the only difference is when they
/// get committed to the database.
///
/// For pending, it is an append operation, but for sidechains they need to move the current
/// canonical blocks to the tree (by removing them from the database), and commit the sidechain
/// blocks to the database to become the canonical chain (reorg).
///
/// `include_mmd!("docs/mermaid/tree.mmd`")
///
/// # Main functions
/// * [`BlockchainTree::insert_block`]: Connect a block to a chain, execute it, and if valid, insert
///   the block into the tree.
/// * [`BlockchainTree::finalize_block`]: Remove chains that branch off of the now finalized block.
/// * [`BlockchainTree::make_canonical`]: Check if we have the hash of a block that is the current
///   canonical head and commit it to db.
#[derive(Debug)]
pub struct BlockchainTree<N: NodeTypesWithDB, E> {
    /// The state of the tree
    ///
    /// Tracks all the chains, the block indices, and the block buffer.
    state: TreeState,
    /// External components (the database, consensus engine etc.)
    externals: TreeExternals<N, E>,
    /// Tree configuration
    config: BlockchainTreeConfig,
    /// Broadcast channel for canon state changes notifications.
    canon_state_notification_sender: CanonStateNotificationSender,
    /// Metrics for sync stages.
    sync_metrics_tx: Option<MetricEventsSender>,
    /// Metrics for the blockchain tree.
    metrics: TreeMetrics,
}

impl<N: NodeTypesWithDB, E> BlockchainTree<N, E> {
    /// Subscribe to new blocks events.
    ///
    /// Note: Only canonical blocks are emitted by the tree.
    pub fn subscribe_canon_state(&self) -> CanonStateNotifications {
        self.canon_state_notification_sender.subscribe()
    }

    /// Returns a clone of the sender for the canonical state notifications.
    pub fn canon_state_notification_sender(&self) -> CanonStateNotificationSender {
        self.canon_state_notification_sender.clone()
    }
}

impl<N, E> BlockchainTree<N, E>
where
    N: ProviderNodeTypes,
    E: BlockExecutorProvider,
{
    /// Builds the blockchain tree for the node.
    ///
    /// This method configures the blockchain tree, which is a critical component of the node,
    /// responsible for managing the blockchain state, including blocks, transactions, and receipts.
    /// It integrates with the consensus mechanism and the EVM for executing transactions.
    ///
    /// # Parameters
    /// - `externals`: External components required by the blockchain tree:
    ///     - `provider_factory`: A factory for creating various blockchain-related providers, such
    ///       as for accessing the database or static files.
    ///     - `consensus`: The consensus configuration, which defines how the node reaches agreement
    ///       on the blockchain state with other nodes.
    ///     - `evm_config`: The EVM (Ethereum Virtual Machine) configuration, which affects how
    ///       smart contracts and transactions are executed. Proper validation of this configuration
    ///       is crucial for the correct execution of transactions.
    /// - `tree_config`: Configuration for the blockchain tree, including any parameters that affect
    ///   its structure or performance.
    pub fn new(
        externals: TreeExternals<N, E>,
        config: BlockchainTreeConfig,
    ) -> ProviderResult<Self> {
        let max_reorg_depth = config.max_reorg_depth() as usize;
        // The size of the broadcast is twice the maximum reorg depth, because at maximum reorg
        // depth at least N blocks must be sent at once.
        let (canon_state_notification_sender, _receiver) =
            tokio::sync::broadcast::channel(max_reorg_depth * 2);

        let last_canonical_hashes =
            externals.fetch_latest_canonical_hashes(config.num_of_canonical_hashes() as usize)?;

        // If we haven't written the finalized block, assume it's zero
        let last_finalized_block_number =
            externals.fetch_latest_finalized_block_number()?.unwrap_or_default();

        Ok(Self {
            externals,
            state: TreeState::new(
                last_finalized_block_number,
                last_canonical_hashes,
                config.max_unconnected_blocks(),
            ),
            config,
            canon_state_notification_sender,
            sync_metrics_tx: None,
            metrics: Default::default(),
        })
    }

    /// Replaces the canon state notification sender.
    ///
    /// Caution: this will close any existing subscriptions to the previous sender.
    #[doc(hidden)]
    pub fn with_canon_state_notification_sender(
        mut self,
        canon_state_notification_sender: CanonStateNotificationSender,
    ) -> Self {
        self.canon_state_notification_sender = canon_state_notification_sender;
        self
    }

    /// Set the sync metric events sender.
    ///
    /// A transmitter for sending synchronization metrics. This is used for monitoring the node's
    /// synchronization process with the blockchain network.
    pub fn with_sync_metrics_tx(mut self, metrics_tx: MetricEventsSender) -> Self {
        self.sync_metrics_tx = Some(metrics_tx);
        self
    }

    /// Check if the block is known to blockchain tree or database and return its status.
    ///
    /// Function will check:
    /// * if block is inside database returns [`BlockStatus::Valid`].
    /// * if block is inside buffer returns [`BlockStatus::Disconnected`].
    /// * if block is part of the canonical returns [`BlockStatus::Valid`].
    ///
    /// Returns an error if
    ///    - an error occurred while reading from the database.
    ///    - the block is already finalized
    pub(crate) fn is_block_known(
        &self,
        block: BlockNumHash,
    ) -> Result<Option<BlockStatus>, InsertBlockErrorKind> {
        // check if block is canonical
        if self.is_block_hash_canonical(&block.hash)? {
            return Ok(Some(BlockStatus::Valid(BlockAttachment::Canonical)));
        }

        let last_finalized_block = self.block_indices().last_finalized_block();
        // check db if block is finalized.
        if block.number <= last_finalized_block {
            // check if block is inside database
            if self.externals.provider_factory.provider()?.block_number(block.hash)?.is_some() {
                return Ok(Some(BlockStatus::Valid(BlockAttachment::Canonical)));
            }

            return Err(BlockchainTreeError::PendingBlockIsFinalized {
                last_finalized: last_finalized_block,
            }
            .into())
        }

        // is block inside chain
        if let Some(attachment) = self.is_block_inside_sidechain(&block) {
            return Ok(Some(BlockStatus::Valid(attachment)));
        }

        // check if block is disconnected
        if let Some(block) = self.state.buffered_blocks.block(&block.hash) {
            return Ok(Some(BlockStatus::Disconnected {
                head: self.state.block_indices.canonical_tip(),
                missing_ancestor: block.parent_num_hash(),
            }))
        }

        Ok(None)
    }

    /// Expose internal indices of the `BlockchainTree`.
    #[inline]
    pub const fn block_indices(&self) -> &BlockIndices {
        self.state.block_indices()
    }

    /// Returns the block with matching hash from any side-chain.
    ///
    /// Caution: This will not return blocks from the canonical chain.
    #[inline]
    pub fn sidechain_block_by_hash(&self, block_hash: BlockHash) -> Option<&SealedBlock> {
        self.state.block_by_hash(block_hash)
    }

    /// Returns the block with matching hash from any side-chain.
    ///
    /// Caution: This will not return blocks from the canonical chain.
    #[inline]
    pub fn block_with_senders_by_hash(
        &self,
        block_hash: BlockHash,
    ) -> Option<&SealedBlockWithSenders> {
        self.state.block_with_senders_by_hash(block_hash)
    }

    /// Returns the block's receipts with matching hash from any side-chain.
    ///
    /// Caution: This will not return blocks from the canonical chain.
    pub fn receipts_by_block_hash(&self, block_hash: BlockHash) -> Option<Vec<&Receipt>> {
        self.state.receipts_by_block_hash(block_hash)
    }

    /// Returns the block that's considered the `Pending` block, if it exists.
    pub fn pending_block(&self) -> Option<&SealedBlock> {
        let b = self.block_indices().pending_block_num_hash()?;
        self.sidechain_block_by_hash(b.hash)
    }

    /// Return items needed to execute on the pending state.
    /// This includes:
    ///     * `BlockHash` of canonical block that chain connects to. Needed for creating database
    ///       provider for the rest of the state.
    ///     * `BundleState` changes that happened at the asked `block_hash`
    ///     * `BTreeMap<BlockNumber,BlockHash>` list of past pending and canonical hashes, That are
    ///       needed for evm `BLOCKHASH` opcode.
    /// Return none if:
    ///     * block unknown.
    ///     * `chain_id` not present in state.
    ///     * there are no parent hashes stored.
    pub fn post_state_data(&self, block_hash: BlockHash) -> Option<ExecutionData> {
        trace!(target: "blockchain_tree", ?block_hash, "Searching for post state data");

        let canonical_chain = self.state.block_indices.canonical_chain();

        // if it is part of the chain
        if let Some(chain_id) = self.block_indices().get_side_chain_id(&block_hash) {
            trace!(target: "blockchain_tree", ?block_hash, "Constructing post state data based on non-canonical chain");
            // get block state
            let Some(chain) = self.state.chains.get(&chain_id) else {
                debug!(target: "blockchain_tree", ?chain_id, "Chain with ID not present");
                return None;
            };
            let block_number = chain.block_number(block_hash)?;
            let execution_outcome = chain.execution_outcome_at_block(block_number)?;

            // get parent hashes
            let mut parent_block_hashes = self.all_chain_hashes(chain_id);
            let Some((first_pending_block_number, _)) = parent_block_hashes.first_key_value()
            else {
                debug!(target: "blockchain_tree", ?chain_id, "No block hashes stored");
                return None;
            };
            let canonical_chain = canonical_chain
                .iter()
                .filter(|&(key, _)| &key < first_pending_block_number)
                .collect::<Vec<_>>();
            parent_block_hashes.extend(canonical_chain);

            // get canonical fork.
            let canonical_fork = self.canonical_fork(chain_id)?;
            return Some(ExecutionData { execution_outcome, parent_block_hashes, canonical_fork });
        }

        // check if there is canonical block
        if let Some(canonical_number) = canonical_chain.canonical_number(&block_hash) {
            trace!(target: "blockchain_tree", %block_hash, "Constructing post state data based on canonical chain");
            return Some(ExecutionData {
                canonical_fork: ForkBlock { number: canonical_number, hash: block_hash },
                execution_outcome: ExecutionOutcome::default(),
                parent_block_hashes: canonical_chain.inner().clone(),
            });
        }

        None
    }

    /// Try inserting a validated [Self::validate_block] block inside the tree.
    ///
    /// If the block's parent block is unknown, this returns [`BlockStatus::Disconnected`] and the
    /// block will be buffered until the parent block is inserted and then attached to sidechain
    #[instrument(level = "trace", skip_all, fields(block = ?block.num_hash()), target = "blockchain_tree", ret)]
    fn try_insert_validated_block(
        &mut self,
        block: SealedBlockWithSenders,
        block_validation_kind: BlockValidationKind,
    ) -> Result<BlockStatus, InsertBlockErrorKind> {
        debug_assert!(self.validate_block(&block).is_ok(), "Block must be validated");

        let parent = block.parent_num_hash();

        // check if block parent can be found in any side chain.
        if let Some(chain_id) = self.block_indices().get_side_chain_id(&parent.hash) {
            // found parent in side tree, try to insert there
            return self.try_insert_block_into_side_chain(block, chain_id, block_validation_kind);
        }

        // if not found, check if the parent can be found inside canonical chain.
        if self.is_block_hash_canonical(&parent.hash)? {
            return self.try_append_canonical_chain(block.clone(), block_validation_kind);
        }

        // this is another check to ensure that if the block points to a canonical block its block
        // is valid
        if let Some(canonical_parent_number) =
            self.block_indices().canonical_number(&block.parent_hash)
        {
            // we found the parent block in canonical chain
            if canonical_parent_number != parent.number {
                return Err(ConsensusError::ParentBlockNumberMismatch {
                    parent_block_number: canonical_parent_number,
                    block_number: block.number,
                }
                .into())
            }
        }

        // if there is a parent inside the buffer, validate against it.
        if let Some(buffered_parent) = self.state.buffered_blocks.block(&parent.hash) {
            self.externals.consensus.validate_header_against_parent(&block, buffered_parent)?;
        }

        // insert block inside unconnected block buffer. Delaying its execution.
        self.state.buffered_blocks.insert_block(block.clone());

        let block_hash = block.hash();
        // find the lowest ancestor of the block in the buffer to return as the missing parent
        // this shouldn't return None because that only happens if the block was evicted, which
        // shouldn't happen right after insertion
        let lowest_ancestor = self
            .state
            .buffered_blocks
            .lowest_ancestor(&block_hash)
            .ok_or(BlockchainTreeError::BlockBufferingFailed { block_hash })?;

        Ok(BlockStatus::Disconnected {
            head: self.state.block_indices.canonical_tip(),
            missing_ancestor: lowest_ancestor.parent_num_hash(),
        })
    }

    /// This tries to append the given block to the canonical chain.
    ///
    /// WARNING: this expects that the block extends the canonical chain: The block's parent is
    /// part of the canonical chain (e.g. the block's parent is the latest canonical hash). See also
    /// [Self::is_block_hash_canonical].
    #[instrument(level = "trace", skip_all, target = "blockchain_tree")]
    fn try_append_canonical_chain(
        &mut self,
        block: SealedBlockWithSenders,
        block_validation_kind: BlockValidationKind,
    ) -> Result<BlockStatus, InsertBlockErrorKind> {
        let parent = block.parent_num_hash();
        let block_num_hash = block.num_hash();
        debug!(target: "blockchain_tree", head = ?block_num_hash.hash, ?parent, "Appending block to canonical chain");

        let provider = self.externals.provider_factory.provider()?;

        // Validate that the block is post merge
        let parent_td = provider
            .header_td(&block.parent_hash)?
            .ok_or_else(|| BlockchainTreeError::CanonicalChain { block_hash: block.parent_hash })?;

        // Pass the parent total difficulty to short-circuit unnecessary calculations.
        if !self
            .externals
            .provider_factory
            .chain_spec()
            .fork(EthereumHardfork::Paris)
            .active_at_ttd(parent_td, U256::ZERO)
        {
            return Err(BlockExecutionError::Validation(BlockValidationError::BlockPreMerge {
                hash: block.hash(),
            })
            .into())
        }

        let parent_header = provider
            .header(&block.parent_hash)?
            .ok_or_else(|| BlockchainTreeError::CanonicalChain { block_hash: block.parent_hash })?;

        let parent_sealed_header = SealedHeader::new(parent_header, block.parent_hash);

        let canonical_chain = self.state.block_indices.canonical_chain();

        let block_attachment = if block.parent_hash == canonical_chain.tip().hash {
            BlockAttachment::Canonical
        } else {
            BlockAttachment::HistoricalFork
        };

        let chain = AppendableChain::new_canonical_fork(
            block,
            &parent_sealed_header,
            canonical_chain.inner(),
            parent,
            &self.externals,
            block_attachment,
            block_validation_kind,
        )?;

        self.insert_chain(chain);
        self.try_connect_buffered_blocks(block_num_hash);

        Ok(BlockStatus::Valid(block_attachment))
    }

    /// Try inserting a block into the given side chain.
    ///
    /// WARNING: This expects a valid side chain id, see [BlockIndices::get_side_chain_id]
    #[instrument(level = "trace", skip_all, target = "blockchain_tree")]
    fn try_insert_block_into_side_chain(
        &mut self,
        block: SealedBlockWithSenders,
        chain_id: SidechainId,
        block_validation_kind: BlockValidationKind,
    ) -> Result<BlockStatus, InsertBlockErrorKind> {
        let block_num_hash = block.num_hash();
        debug!(target: "blockchain_tree", ?block_num_hash, ?chain_id, "Inserting block into side chain");
        // Create a new sidechain by forking the given chain, or append the block if the parent
        // block is the top of the given chain.
        let block_hashes = self.all_chain_hashes(chain_id);

        // get canonical fork.
        let canonical_fork = self.canonical_fork(chain_id).ok_or_else(|| {
            BlockchainTreeError::BlockSideChainIdConsistency { chain_id: chain_id.into() }
        })?;

        // get chain that block needs to join to.
        let parent_chain = self.state.chains.get_mut(&chain_id).ok_or_else(|| {
            BlockchainTreeError::BlockSideChainIdConsistency { chain_id: chain_id.into() }
        })?;

        let chain_tip = parent_chain.tip().hash();
        let canonical_chain = self.state.block_indices.canonical_chain();

        // append the block if it is continuing the side chain.
        let block_attachment = if chain_tip == block.parent_hash {
            // check if the chain extends the currently tracked canonical head
            let block_attachment = if canonical_fork.hash == canonical_chain.tip().hash {
                BlockAttachment::Canonical
            } else {
                BlockAttachment::HistoricalFork
            };

            let block_hash = block.hash();
            let block_number = block.number;
            debug!(target: "blockchain_tree", ?block_hash, ?block_number, "Appending block to side chain");
            parent_chain.append_block(
                block,
                block_hashes,
                canonical_chain.inner(),
                &self.externals,
                canonical_fork,
                block_attachment,
                block_validation_kind,
            )?;

            self.state.block_indices.insert_non_fork_block(block_number, block_hash, chain_id);
            block_attachment
        } else {
            debug!(target: "blockchain_tree", ?canonical_fork, "Starting new fork from side chain");
            // the block starts a new fork
            let chain = parent_chain.new_chain_fork(
                block,
                block_hashes,
                canonical_chain.inner(),
                canonical_fork,
                &self.externals,
                block_validation_kind,
            )?;
            self.insert_chain(chain);
            BlockAttachment::HistoricalFork
        };

        // After we inserted the block, we try to connect any buffered blocks
        self.try_connect_buffered_blocks(block_num_hash);

        Ok(BlockStatus::Valid(block_attachment))
    }

    /// Get all block hashes from a sidechain that are not part of the canonical chain.
    /// This is a one time operation per block.
    ///
    /// # Note
    ///
    /// This is not cached in order to save memory.
    fn all_chain_hashes(&self, chain_id: SidechainId) -> BTreeMap<BlockNumber, BlockHash> {
        let mut chain_id = chain_id;
        let mut hashes = BTreeMap::new();
        loop {
            let Some(chain) = self.state.chains.get(&chain_id) else { return hashes };

            // The parent chains might contain blocks with overlapping numbers or numbers greater
            // than original chain tip. Insert the block hash only if it's not present
            // for the given block number and the block number does not exceed the
            // original chain tip.
            let latest_block_number = hashes
                .last_key_value()
                .map(|(number, _)| *number)
                .unwrap_or_else(|| chain.tip().number);
            for block in chain.blocks().values().filter(|b| b.number <= latest_block_number) {
                if let Entry::Vacant(e) = hashes.entry(block.number) {
                    e.insert(block.hash());
                }
            }

            let fork_block = chain.fork_block();
            if let Some(next_chain_id) = self.block_indices().get_side_chain_id(&fork_block.hash) {
                chain_id = next_chain_id;
            } else {
                // if there is no fork block that point to other chains, break the loop.
                // it means that this fork joins to canonical block.
                break
            }
        }
        hashes
    }

    /// Get the block at which the given chain forks off the current canonical chain.
    ///
    /// This is used to figure out what kind of state provider the executor should use to execute
    /// the block on
    ///
    /// Returns `None` if the chain is unknown.
    fn canonical_fork(&self, chain_id: SidechainId) -> Option<ForkBlock> {
        let mut chain_id = chain_id;
        let mut fork;
        loop {
            // chain fork block
            fork = self.state.chains.get(&chain_id)?.fork_block();
            // get fork block chain
            if let Some(fork_chain_id) = self.block_indices().get_side_chain_id(&fork.hash) {
                chain_id = fork_chain_id;
                continue
            }
            break
        }
        (self.block_indices().canonical_hash(&fork.number) == Some(fork.hash)).then_some(fork)
    }

    /// Insert a chain into the tree.
    ///
    /// Inserts a chain into the tree and builds the block indices.
    fn insert_chain(&mut self, chain: AppendableChain) -> Option<SidechainId> {
        self.state.insert_chain(chain)
    }

    /// Iterate over all child chains that depend on this block and return
    /// their ids.
    fn find_all_dependent_chains(&self, block: &BlockHash) -> HashSet<SidechainId> {
        // Find all forks of given block.
        let mut dependent_block =
            self.block_indices().fork_to_child().get(block).cloned().unwrap_or_default();
        let mut dependent_chains = HashSet::default();

        while let Some(block) = dependent_block.pop_back() {
            // Get chain of dependent block.
            let Some(chain_id) = self.block_indices().get_side_chain_id(&block) else {
                debug!(target: "blockchain_tree", ?block, "Block not in tree");
                return Default::default();
            };

            // Find all blocks that fork from this chain.
            let Some(chain) = self.state.chains.get(&chain_id) else {
                debug!(target: "blockchain_tree", ?chain_id, "Chain not in tree");
                return Default::default();
            };
            for chain_block in chain.blocks().values() {
                if let Some(forks) = self.block_indices().fork_to_child().get(&chain_block.hash()) {
                    // If there are sub forks append them for processing.
                    dependent_block.extend(forks);
                }
            }
            // Insert dependent chain id.
            dependent_chains.insert(chain_id);
        }
        dependent_chains
    }

    /// Inserts unwound chain back into the tree and updates any dependent chains.
    ///
    /// This method searches for any chain that depended on this block being part of the canonical
    /// chain. Each dependent chain's state is then updated with state entries removed from the
    /// plain state during the unwind.
    /// Returns the result of inserting the chain or None if any of the dependent chains is not
    /// in the tree.
    fn insert_unwound_chain(&mut self, chain: AppendableChain) -> Option<SidechainId> {
        // iterate over all blocks in chain and find any fork blocks that are in tree.
        for (number, block) in chain.blocks() {
            let hash = block.hash();

            // find all chains that fork from this block.
            let chains_to_bump = self.find_all_dependent_chains(&hash);
            if !chains_to_bump.is_empty() {
                // if there is such chain, revert state to this block.
                let mut cloned_execution_outcome = chain.execution_outcome().clone();
                cloned_execution_outcome.revert_to(*number);

                // prepend state to all chains that fork from this block.
                for chain_id in chains_to_bump {
                    let Some(chain) = self.state.chains.get_mut(&chain_id) else {
                        debug!(target: "blockchain_tree", ?chain_id, "Chain not in tree");
                        return None;
                    };

                    debug!(target: "blockchain_tree",
                        unwound_block= ?block.num_hash(),
                        chain_id = ?chain_id,
                        chain_tip = ?chain.tip().num_hash(),
                        "Prepend unwound block state to blockchain tree chain");

                    chain.prepend_state(cloned_execution_outcome.state().clone())
                }
            }
        }
        // Insert unwound chain to the tree.
        self.insert_chain(chain)
    }

    /// Checks the block buffer for the given block.
    pub fn get_buffered_block(&self, hash: &BlockHash) -> Option<&SealedBlockWithSenders> {
        self.state.get_buffered_block(hash)
    }

    /// Gets the lowest ancestor for the given block in the block buffer.
    pub fn lowest_buffered_ancestor(&self, hash: &BlockHash) -> Option<&SealedBlockWithSenders> {
        self.state.lowest_buffered_ancestor(hash)
    }

    /// Insert a new block into the tree.
    ///
    /// # Note
    ///
    /// This recovers transaction signers (unlike [`BlockchainTree::insert_block`]).
    pub fn insert_block_without_senders(
        &mut self,
        block: SealedBlock,
    ) -> Result<InsertPayloadOk, InsertBlockError> {
        match block.try_seal_with_senders() {
            Ok(block) => self.insert_block(block, BlockValidationKind::Exhaustive),
            Err(block) => Err(InsertBlockError::sender_recovery_error(block)),
        }
    }

    /// Insert block for future execution.
    ///
    /// Returns an error if the block is invalid.
    pub fn buffer_block(&mut self, block: SealedBlockWithSenders) -> Result<(), InsertBlockError> {
        // validate block consensus rules
        if let Err(err) = self.validate_block(&block) {
            return Err(InsertBlockError::consensus_error(err, block.block));
        }

        self.state.buffered_blocks.insert_block(block);
        Ok(())
    }

    /// Validate if block is correct and satisfies all the consensus rules that concern the header
    /// and block body itself.
    fn validate_block(&self, block: &SealedBlockWithSenders) -> Result<(), ConsensusError> {
        if let Err(e) =
            self.externals.consensus.validate_header_with_total_difficulty(block, U256::MAX)
        {
            error!(
                ?block,
                "Failed to validate total difficulty for block {}: {e}",
                block.header.hash()
            );
            return Err(e);
        }

        if let Err(e) = self.externals.consensus.validate_header(block) {
            error!(?block, "Failed to validate header {}: {e}", block.header.hash());
            return Err(e);
        }

        if let Err(e) = self.externals.consensus.validate_block_pre_execution(block) {
            error!(?block, "Failed to validate block {}: {e}", block.header.hash());
            return Err(e);
        }

        Ok(())
    }

    /// Check if block is found inside a sidechain and its attachment.
    ///
    /// if it is canonical or extends the canonical chain, return [`BlockAttachment::Canonical`]
    /// if it does not extend the canonical chain, return [`BlockAttachment::HistoricalFork`]
    /// if the block is not in the tree or its chain id is not valid, return None
    #[track_caller]
    fn is_block_inside_sidechain(&self, block: &BlockNumHash) -> Option<BlockAttachment> {
        // check if block known and is already in the tree
        if let Some(chain_id) = self.block_indices().get_side_chain_id(&block.hash) {
            // find the canonical fork of this chain
            let Some(canonical_fork) = self.canonical_fork(chain_id) else {
                debug!(target: "blockchain_tree", chain_id=?chain_id, block=?block.hash, "Chain id not valid");
                return None;
            };
            // if the block's chain extends canonical chain
            return if canonical_fork == self.block_indices().canonical_tip() {
                Some(BlockAttachment::Canonical)
            } else {
                Some(BlockAttachment::HistoricalFork)
            };
        }
        None
    }

    /// Insert a block (with recovered senders) into the tree.
    ///
    /// Returns the [`BlockStatus`] on success:
    ///
    /// - The block is already part of a sidechain in the tree, or
    /// - The block is already part of the canonical chain, or
    /// - The parent is part of a sidechain in the tree, and we can fork at this block, or
    /// - The parent is part of the canonical chain, and we can fork at this block
    ///
    /// Otherwise, an error is returned, indicating that neither the block nor its parent are part
    /// of the chain or any sidechains.
    ///
    /// This means that if the block becomes canonical, we need to fetch the missing blocks over
    /// P2P.
    ///
    /// If the [`BlockValidationKind::SkipStateRootValidation`] variant is provided the state root
    /// is not validated.
    ///
    /// # Note
    ///
    /// If the senders have not already been recovered, call
    /// [`BlockchainTree::insert_block_without_senders`] instead.
    pub fn insert_block(
        &mut self,
        block: SealedBlockWithSenders,
        block_validation_kind: BlockValidationKind,
    ) -> Result<InsertPayloadOk, InsertBlockError> {
        // check if we already have this block
        match self.is_block_known(block.num_hash()) {
            Ok(Some(status)) => return Ok(InsertPayloadOk::AlreadySeen(status)),
            Err(err) => return Err(InsertBlockError::new(block.block, err)),
            _ => {}
        }

        // validate block consensus rules
        if let Err(err) = self.validate_block(&block) {
            return Err(InsertBlockError::consensus_error(err, block.block));
        }

        let status = self
            .try_insert_validated_block(block.clone(), block_validation_kind)
            .map_err(|kind| InsertBlockError::new(block.block, kind))?;
        Ok(InsertPayloadOk::Inserted(status))
    }

    /// Discard all blocks that precede block number from the buffer.
    pub fn remove_old_blocks(&mut self, block: BlockNumber) {
        self.state.buffered_blocks.remove_old_blocks(block);
    }

    /// Finalize blocks up until and including `finalized_block`, and remove them from the tree.
    pub fn finalize_block(&mut self, finalized_block: BlockNumber) -> ProviderResult<()> {
        // remove blocks
        let mut remove_chains = self.state.block_indices.finalize_canonical_blocks(
            finalized_block,
            self.config.num_of_additional_canonical_block_hashes(),
        );
        // remove chains of removed blocks
        while let Some(chain_id) = remove_chains.pop_first() {
            if let Some(chain) = self.state.chains.remove(&chain_id) {
                remove_chains.extend(self.state.block_indices.remove_chain(&chain));
            }
        }
        // clean block buffer.
        self.remove_old_blocks(finalized_block);

        // save finalized block in db.
        self.externals.save_finalized_block_number(finalized_block)?;

        Ok(())
    }

    /// Reads the last `N` canonical hashes from the database and updates the block indices of the
    /// tree by attempting to connect the buffered blocks to canonical hashes.
    ///
    ///
    /// `N` is the maximum of `max_reorg_depth` and the number of block hashes needed to satisfy the
    /// `BLOCKHASH` opcode in the EVM.
    ///
    /// # Note
    ///
    /// This finalizes `last_finalized_block` prior to reading the canonical hashes (using
    /// [`BlockchainTree::finalize_block`]).
    pub fn connect_buffered_blocks_to_canonical_hashes_and_finalize(
        &mut self,
        last_finalized_block: BlockNumber,
    ) -> ProviderResult<()> {
        self.finalize_block(last_finalized_block)?;

        let last_canonical_hashes = self.update_block_hashes()?;

        self.connect_buffered_blocks_to_hashes(last_canonical_hashes)?;

        Ok(())
    }

    /// Update all block hashes. iterate over present and new list of canonical hashes and compare
    /// them. Remove all mismatches, disconnect them and removes all chains.
    pub fn update_block_hashes(&mut self) -> ProviderResult<BTreeMap<BlockNumber, B256>> {
        let last_canonical_hashes = self
            .externals
            .fetch_latest_canonical_hashes(self.config.num_of_canonical_hashes() as usize)?;

        let (mut remove_chains, _) =
            self.state.block_indices.update_block_hashes(last_canonical_hashes.clone());

        // remove all chains that got discarded
        while let Some(chain_id) = remove_chains.first() {
            if let Some(chain) = self.state.chains.remove(chain_id) {
                remove_chains.extend(self.state.block_indices.remove_chain(&chain));
            }
        }

        Ok(last_canonical_hashes)
    }

    /// Update all block hashes. iterate over present and new list of canonical hashes and compare
    /// them. Remove all mismatches, disconnect them, removes all chains and clears all buffered
    /// blocks before the tip.
    pub fn update_block_hashes_and_clear_buffered(
        &mut self,
    ) -> ProviderResult<BTreeMap<BlockNumber, BlockHash>> {
        let chain = self.update_block_hashes()?;

        if let Some((block, _)) = chain.last_key_value() {
            self.remove_old_blocks(*block);
        }

        Ok(chain)
    }

    /// Reads the last `N` canonical hashes from the database and updates the block indices of the
    /// tree by attempting to connect the buffered blocks to canonical hashes.
    ///
    /// `N` is the maximum of `max_reorg_depth` and the number of block hashes needed to satisfy the
    /// `BLOCKHASH` opcode in the EVM.
    pub fn connect_buffered_blocks_to_canonical_hashes(&mut self) -> ProviderResult<()> {
        let last_canonical_hashes = self
            .externals
            .fetch_latest_canonical_hashes(self.config.num_of_canonical_hashes() as usize)?;
        self.connect_buffered_blocks_to_hashes(last_canonical_hashes)?;

        Ok(())
    }

    fn connect_buffered_blocks_to_hashes(
        &mut self,
        hashes: impl IntoIterator<Item = impl Into<BlockNumHash>>,
    ) -> ProviderResult<()> {
        // check unconnected block buffer for children of the canonical hashes
        for added_block in hashes {
            self.try_connect_buffered_blocks(added_block.into())
        }

        // check unconnected block buffer for children of the chains
        let mut all_chain_blocks = Vec::new();
        for chain in self.state.chains.values() {
            all_chain_blocks.reserve_exact(chain.blocks().len());
            for (&number, block) in chain.blocks() {
                all_chain_blocks.push(BlockNumHash { number, hash: block.hash() })
            }
        }
        for block in all_chain_blocks {
            self.try_connect_buffered_blocks(block)
        }

        Ok(())
    }

    /// Connect unconnected (buffered) blocks if the new block closes a gap.
    ///
    /// This will try to insert all children of the new block, extending its chain.
    ///
    /// If all children are valid, then this essentially appends all child blocks to the
    /// new block's chain.
    fn try_connect_buffered_blocks(&mut self, new_block: BlockNumHash) {
        trace!(target: "blockchain_tree", ?new_block, "try_connect_buffered_blocks");

        // first remove all the children of the new block from the buffer
        let include_blocks = self.state.buffered_blocks.remove_block_with_children(&new_block.hash);
        // then try to reinsert them into the tree
        for block in include_blocks {
            // don't fail on error, just ignore the block.
            let _ = self
                .try_insert_validated_block(block, BlockValidationKind::SkipStateRootValidation)
                .map_err(|err| {
                    debug!(target: "blockchain_tree", %err, "Failed to insert buffered block");
                    err
                });
        }
    }

    /// Removes chain corresponding to provided chain id from block indices,
    /// splits it at split target, and returns the canonical part of it.
    /// Returns [None] if chain is missing.
    ///
    /// The pending part of the chain is reinserted back into the tree with the same `chain_id`.
    fn remove_and_split_chain(
        &mut self,
        chain_id: SidechainId,
        split_at: ChainSplitTarget,
    ) -> Option<Chain> {
        let chain = self.state.chains.remove(&chain_id)?;
        match chain.into_inner().split(split_at) {
            ChainSplit::Split { canonical, pending } => {
                trace!(target: "blockchain_tree", ?canonical, ?pending, "Split chain");
                // rest of split chain is inserted back with same chain_id.
                self.state.block_indices.insert_chain(chain_id, &pending);
                self.state.chains.insert(chain_id, AppendableChain::new(pending));
                Some(canonical)
            }
            ChainSplit::NoSplitCanonical(canonical) => {
                trace!(target: "blockchain_tree", "No split on canonical chain");
                Some(canonical)
            }
            ChainSplit::NoSplitPending(_) => {
                unreachable!("Should not happen as block indices guarantee structure of blocks")
            }
        }
    }

    /// Attempts to find the header for the given block hash if it is canonical.
    ///
    /// Returns `Ok(None)` if the block hash is not canonical (block hash does not exist, or is
    /// included in a sidechain).
    ///
    /// Note: this does not distinguish between a block that is finalized and a block that is not
    /// finalized yet, only whether it is part of the canonical chain or not.
    pub fn find_canonical_header(
        &self,
        hash: &BlockHash,
    ) -> Result<Option<SealedHeader>, ProviderError> {
        // if the indices show that the block hash is not canonical, it's either in a sidechain or
        // canonical, but in the db. If it is in a sidechain, it is not canonical. If it is missing
        // in the db, then it is also not canonical.

        let provider = self.externals.provider_factory.provider()?;

        let mut header = None;
        if let Some(num) = self.block_indices().canonical_number(hash) {
            header = provider.header_by_number(num)?;
        }

        if header.is_none() && self.sidechain_block_by_hash(*hash).is_some() {
            return Ok(None)
        }

        if header.is_none() {
            header = provider.header(hash)?
        }

        Ok(header.map(|header| SealedHeader::new(header, *hash)))
    }

    /// Determines whether or not a block is canonical, checking the db if necessary.
    ///
    /// Note: this does not distinguish between a block that is finalized and a block that is not
    /// finalized yet, only whether it is part of the canonical chain or not.
    pub fn is_block_hash_canonical(&self, hash: &BlockHash) -> Result<bool, ProviderError> {
        self.find_canonical_header(hash).map(|header| header.is_some())
    }

    /// Make a block and its parent(s) part of the canonical chain and commit them to the database
    ///
    /// # Note
    ///
    /// This unwinds the database if necessary, i.e. if parts of the canonical chain have been
    /// reorged.
    ///
    /// # Returns
    ///
    /// Returns `Ok` if the blocks were canonicalized, or if the blocks were already canonical.
    #[track_caller]
    #[instrument(level = "trace", skip(self), target = "blockchain_tree")]
    pub fn make_canonical(
        &mut self,
        block_hash: BlockHash,
    ) -> Result<CanonicalOutcome, CanonicalError> {
        let mut durations_recorder = MakeCanonicalDurationsRecorder::default();

        let old_block_indices = self.block_indices().clone();
        let old_buffered_blocks = self.state.buffered_blocks.parent_to_child.clone();
        durations_recorder.record_relative(MakeCanonicalAction::CloneOldBlocks);

        // If block is already canonical don't return error.
        let canonical_header = self.find_canonical_header(&block_hash)?;
        durations_recorder.record_relative(MakeCanonicalAction::FindCanonicalHeader);
        if let Some(header) = canonical_header {
            info!(target: "blockchain_tree", %block_hash, "Block is already canonical, ignoring.");
            // TODO: this could be fetched from the chainspec first
            let td =
                self.externals.provider_factory.provider()?.header_td(&block_hash)?.ok_or_else(
                    || {
                        CanonicalError::from(BlockValidationError::MissingTotalDifficulty {
                            hash: block_hash,
                        })
                    },
                )?;
            if !self
                .externals
                .provider_factory
                .chain_spec()
                .fork(EthereumHardfork::Paris)
                .active_at_ttd(td, U256::ZERO)
            {
                return Err(CanonicalError::from(BlockValidationError::BlockPreMerge {
                    hash: block_hash,
                }))
            }

            let head = self.state.block_indices.canonical_tip();
            return Ok(CanonicalOutcome::AlreadyCanonical { header, head });
        }

        let Some(chain_id) = self.block_indices().get_side_chain_id(&block_hash) else {
            debug!(target: "blockchain_tree", ?block_hash, "Block hash not found in block indices");
            return Err(CanonicalError::from(BlockchainTreeError::BlockHashNotFoundInChain {
                block_hash,
            }))
        };

        // we are splitting chain at the block hash that we want to make canonical
        let Some(canonical) = self.remove_and_split_chain(chain_id, block_hash.into()) else {
            debug!(target: "blockchain_tree", ?block_hash, ?chain_id, "Chain not present");
            return Err(CanonicalError::from(BlockchainTreeError::BlockSideChainIdConsistency {
                chain_id: chain_id.into(),
            }))
        };
        trace!(target: "blockchain_tree", chain = ?canonical, "Found chain to make canonical");
        durations_recorder.record_relative(MakeCanonicalAction::SplitChain);

        let mut fork_block = canonical.fork_block();
        let mut chains_to_promote = vec![canonical];

        // loop while fork blocks are found in Tree.
        while let Some(chain_id) = self.block_indices().get_side_chain_id(&fork_block.hash) {
            // canonical chain is lower part of the chain.
            let Some(canonical) =
                self.remove_and_split_chain(chain_id, ChainSplitTarget::Number(fork_block.number))
            else {
                debug!(target: "blockchain_tree", ?fork_block, ?chain_id, "Fork not present");
                return Err(CanonicalError::from(
                    BlockchainTreeError::BlockSideChainIdConsistency { chain_id: chain_id.into() },
                ));
            };
            fork_block = canonical.fork_block();
            chains_to_promote.push(canonical);
        }
        durations_recorder.record_relative(MakeCanonicalAction::SplitChainForks);

        let old_tip = self.block_indices().canonical_tip();
        // Merge all chains into one chain.
        let Some(mut new_canon_chain) = chains_to_promote.pop() else {
            debug!(target: "blockchain_tree", "No blocks in the chain to make canonical");
            return Err(CanonicalError::from(BlockchainTreeError::BlockHashNotFoundInChain {
                block_hash: fork_block.hash,
            }))
        };
        trace!(target: "blockchain_tree", ?new_canon_chain, "Merging chains");
        let mut chain_appended = false;
        for chain in chains_to_promote.into_iter().rev() {
            trace!(target: "blockchain_tree", ?chain, "Appending chain");
            let block_hash = chain.fork_block().hash;
            new_canon_chain.append_chain(chain).map_err(|_| {
                CanonicalError::from(BlockchainTreeError::BlockHashNotFoundInChain { block_hash })
            })?;
            chain_appended = true;
        }
        durations_recorder.record_relative(MakeCanonicalAction::MergeAllChains);

        if chain_appended {
            trace!(target: "blockchain_tree", ?new_canon_chain, "Canonical chain appended");
        }
        // update canonical index
        self.state.block_indices.canonicalize_blocks(new_canon_chain.blocks());
        durations_recorder.record_relative(MakeCanonicalAction::UpdateCanonicalIndex);

        debug!(
            target: "blockchain_tree",
            "Committing new canonical chain: {}", DisplayBlocksChain(new_canon_chain.blocks())
        );

        // If chain extends the tip
        let chain_notification = if new_canon_chain.fork_block().hash == old_tip.hash {
            // Commit new canonical chain to database.
            self.commit_canonical_to_database(new_canon_chain.clone(), &mut durations_recorder)?;
            CanonStateNotification::Commit { new: Arc::new(new_canon_chain) }
        } else {
            // It forks to canonical block that is not the tip.
            let canon_fork: BlockNumHash = new_canon_chain.fork_block();
            // sanity check
            if self.block_indices().canonical_hash(&canon_fork.number) != Some(canon_fork.hash) {
                error!(
                    target: "blockchain_tree",
                    ?canon_fork,
                    block_indices=?self.block_indices(),
                    "All chains should point to canonical chain"
                );
                unreachable!("all chains should point to canonical chain.");
            }

            let old_canon_chain =
                self.revert_canonical_from_database(canon_fork.number).inspect_err(|error| {
                    error!(
                        target: "blockchain_tree",
                        "Reverting canonical chain failed with error: {:?}\n\
                            Old BlockIndices are:{:?}\n\
                            New BlockIndices are: {:?}\n\
                            Old BufferedBlocks are:{:?}",
                        error, old_block_indices, self.block_indices(), old_buffered_blocks
                    );
                })?;
            durations_recorder
                .record_relative(MakeCanonicalAction::RevertCanonicalChainFromDatabase);

            // Commit new canonical chain.
            self.commit_canonical_to_database(new_canon_chain.clone(), &mut durations_recorder)?;

            if let Some(old_canon_chain) = old_canon_chain {
                self.update_reorg_metrics(old_canon_chain.len() as f64);

                // Insert old canonical chain back into tree.
                self.insert_unwound_chain(AppendableChain::new(old_canon_chain.clone()));
                durations_recorder.record_relative(MakeCanonicalAction::InsertOldCanonicalChain);

                CanonStateNotification::Reorg {
                    old: Arc::new(old_canon_chain),
                    new: Arc::new(new_canon_chain),
                }
            } else {
                // error here to confirm that we are reverting nothing from db.
                error!(target: "blockchain_tree", %block_hash, "Nothing was removed from database");
                CanonStateNotification::Commit { new: Arc::new(new_canon_chain) }
            }
        };

        debug!(
            target: "blockchain_tree",
            actions = ?durations_recorder.actions,
            "Canonicalization finished"
        );

        // clear trie updates for other children
        self.block_indices()
            .fork_to_child()
            .get(&old_tip.hash)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .for_each(|child| {
                if let Some(chain_id) = self.block_indices().get_side_chain_id(&child) {
                    if let Some(chain) = self.state.chains.get_mut(&chain_id) {
                        chain.clear_trie_updates();
                    }
                }
            });

        durations_recorder.record_relative(MakeCanonicalAction::ClearTrieUpdatesForOtherChildren);

        // Send notification about new canonical chain and return outcome of canonicalization.
        let outcome = CanonicalOutcome::Committed { head: chain_notification.tip().header.clone() };
        let _ = self.canon_state_notification_sender.send(chain_notification);
        Ok(outcome)
    }

    /// Write the given chain to the database as canonical.
    fn commit_canonical_to_database(
        &self,
        chain: Chain,
        recorder: &mut MakeCanonicalDurationsRecorder,
    ) -> Result<(), CanonicalError> {
        let (blocks, state, chain_trie_updates) = chain.into_inner();
        let hashed_state = state.hash_state_slow();
        let prefix_sets = hashed_state.construct_prefix_sets().freeze();
        let hashed_state_sorted = hashed_state.into_sorted();

        // Compute state root or retrieve cached trie updates before opening write transaction.
        let block_hash_numbers =
            blocks.iter().map(|(number, b)| (number, b.hash())).collect::<Vec<_>>();
        let trie_updates = match chain_trie_updates {
            Some(updates) => {
                debug!(target: "blockchain_tree", blocks = ?block_hash_numbers, "Using cached trie updates");
                self.metrics.trie_updates_insert_cached.increment(1);
                updates
            }
            None => {
                debug!(target: "blockchain_tree", blocks = ?block_hash_numbers, "Recomputing state root for insert");
                let provider = self
                    .externals
                    .provider_factory
                    .provider()?
                    // State root calculation can take a while, and we're sure no write transaction
                    // will be open in parallel. See https://github.com/paradigmxyz/reth/issues/6168.
                    .disable_long_read_transaction_safety();
                let (state_root, trie_updates) = StateRoot::from_tx(provider.tx_ref())
                    .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                        DatabaseHashedCursorFactory::new(provider.tx_ref()),
                        &hashed_state_sorted,
                    ))
                    .with_prefix_sets(prefix_sets)
                    .root_with_updates()
                    .map_err(Into::<BlockValidationError>::into)?;
                let tip = blocks.tip();
                if state_root != tip.state_root {
                    return Err(ProviderError::StateRootMismatch(Box::new(RootMismatch {
                        root: GotExpected { got: state_root, expected: tip.state_root },
                        block_number: tip.number,
                        block_hash: tip.hash(),
                    }))
                    .into())
                }
                self.metrics.trie_updates_insert_recomputed.increment(1);
                trie_updates
            }
        };
        recorder.record_relative(MakeCanonicalAction::RetrieveStateTrieUpdates);

        let provider_rw = self.externals.provider_factory.provider_rw()?;
        provider_rw
            .append_blocks_with_state(
                blocks.into_blocks().collect(),
                state,
                hashed_state_sorted,
                trie_updates,
            )
            .map_err(|e| CanonicalError::CanonicalCommit(e.to_string()))?;

        provider_rw.commit()?;
        recorder.record_relative(MakeCanonicalAction::CommitCanonicalChainToDatabase);

        Ok(())
    }

    /// Unwind tables and put it inside state
    pub fn unwind(&mut self, unwind_to: BlockNumber) -> Result<(), CanonicalError> {
        // nothing to be done if unwind_to is higher then the tip
        if self.block_indices().canonical_tip().number <= unwind_to {
            return Ok(());
        }
        // revert `N` blocks from current canonical chain and put them inside BlockchainTree
        let old_canon_chain = self.revert_canonical_from_database(unwind_to)?;

        // check if there is block in chain
        if let Some(old_canon_chain) = old_canon_chain {
            self.state.block_indices.unwind_canonical_chain(unwind_to);
            // insert old canonical chain to BlockchainTree.
            self.insert_unwound_chain(AppendableChain::new(old_canon_chain));
        }

        Ok(())
    }

    /// Reverts the canonical chain down to the given block from the database and returns the
    /// unwound chain.
    ///
    /// The block, `revert_until`, is __non-inclusive__, i.e. `revert_until` stays in the database.
    fn revert_canonical_from_database(
        &self,
        revert_until: BlockNumber,
    ) -> Result<Option<Chain>, CanonicalError> {
        // This should only happen when an optimistic sync target was re-orged.
        //
        // Static files generally contain finalized data. The blockchain tree only deals
        // with non-finalized data. The only scenario where canonical reverts go past the highest
        // static file is when an optimistic sync occurred and non-finalized data was written to
        // static files.
        if self
            .externals
            .provider_factory
            .static_file_provider()
            .get_highest_static_file_block(StaticFileSegment::Headers)
            .unwrap_or_default() >
            revert_until
        {
            trace!(
                target: "blockchain_tree",
                "Reverting optimistic canonical chain to block {}",
                revert_until
            );
            return Err(CanonicalError::OptimisticTargetRevert(revert_until));
        }

        // read data that is needed for new sidechain
        let provider_rw = self.externals.provider_factory.provider_rw()?;

        let tip = provider_rw.last_block_number()?;
        let revert_range = (revert_until + 1)..=tip;
        info!(target: "blockchain_tree", "REORG: revert canonical from database by unwinding chain blocks {:?}", revert_range);
        // read block and execution result from database. and remove traces of block from tables.
        let blocks_and_execution = provider_rw
            .take_block_and_execution_range(revert_range)
            .map_err(|e| CanonicalError::CanonicalRevert(e.to_string()))?;

        provider_rw.commit()?;

        if blocks_and_execution.is_empty() {
            Ok(None)
        } else {
            Ok(Some(blocks_and_execution))
        }
    }

    fn update_reorg_metrics(&self, reorg_depth: f64) {
        self.metrics.reorgs.increment(1);
        self.metrics.latest_reorg_depth.set(reorg_depth);
    }

    /// Update blockchain tree chains (canonical and sidechains) and sync metrics.
    ///
    /// NOTE: this method should not be called during the pipeline sync, because otherwise the sync
    /// checkpoint metric will get overwritten. Buffered blocks metrics are updated in
    /// [`BlockBuffer`](crate::block_buffer::BlockBuffer) during the pipeline sync.
    pub(crate) fn update_chains_metrics(&mut self) {
        let height = self.state.block_indices.canonical_tip().number;

        let longest_sidechain_height =
            self.state.chains.values().map(|chain| chain.tip().number).max();
        if let Some(longest_sidechain_height) = longest_sidechain_height {
            self.metrics.longest_sidechain_height.set(longest_sidechain_height as f64);
        }

        self.metrics.sidechains.set(self.state.chains.len() as f64);
        self.metrics.canonical_chain_height.set(height as f64);
        if let Some(metrics_tx) = self.sync_metrics_tx.as_mut() {
            let _ = metrics_tx.send(MetricEvent::SyncHeight { height });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{TxEip1559, EMPTY_ROOT_HASH};
    use alloy_genesis::{Genesis, GenesisAccount};
    use alloy_primitives::{keccak256, Address, Sealable, B256};
    use assert_matches::assert_matches;
    use linked_hash_set::LinkedHashSet;
    use reth_chainspec::{ChainSpecBuilder, MAINNET, MIN_TRANSACTION_GAS};
    use reth_consensus::test_utils::TestConsensus;
    use reth_db::tables;
    use reth_db_api::transaction::DbTxMut;
    use reth_evm::test_utils::MockExecutorProvider;
    use reth_evm_ethereum::execute::EthExecutorProvider;
    use reth_primitives::{
        constants::EIP1559_INITIAL_BASE_FEE,
        proofs::{calculate_receipt_root, calculate_transaction_root},
        revm_primitives::AccountInfo,
        Account, BlockBody, Header, Signature, Transaction, TransactionSigned,
        TransactionSignedEcRecovered, Withdrawals,
    };
    use reth_provider::{
        test_utils::{
            blocks::BlockchainTestData, create_test_provider_factory_with_chain_spec,
            MockNodeTypesWithDB,
        },
        ProviderFactory,
    };
    use reth_stages_api::StageCheckpoint;
    use reth_trie::{root::state_root_unhashed, StateRoot};
    use std::collections::HashMap;

    fn setup_externals(
        exec_res: Vec<ExecutionOutcome>,
    ) -> TreeExternals<MockNodeTypesWithDB, MockExecutorProvider> {
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .shanghai_activated()
                .build(),
        );
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec);
        let consensus = Arc::new(TestConsensus::default());
        let executor_factory = MockExecutorProvider::default();
        executor_factory.extend(exec_res);

        TreeExternals::new(provider_factory, consensus, executor_factory)
    }

    fn setup_genesis<N: ProviderNodeTypes>(factory: &ProviderFactory<N>, mut genesis: SealedBlock) {
        // insert genesis to db.

        genesis.header.set_block_number(10);
        genesis.header.set_state_root(EMPTY_ROOT_HASH);
        let provider = factory.provider_rw().unwrap();

        provider
            .insert_historical_block(
                genesis.try_seal_with_senders().expect("invalid tx signature in genesis"),
            )
            .unwrap();

        // insert first 10 blocks
        for i in 0..10 {
            provider
                .tx_ref()
                .put::<tables::CanonicalHeaders>(i, B256::new([100 + i as u8; 32]))
                .unwrap();
        }
        provider
            .tx_ref()
            .put::<tables::StageCheckpoints>("Finish".to_string(), StageCheckpoint::new(10))
            .unwrap();
        provider.commit().unwrap();
    }

    /// Test data structure that will check tree internals
    #[derive(Default, Debug)]
    struct TreeTester {
        /// Number of chains
        chain_num: Option<usize>,
        /// Check block to chain index
        block_to_chain: Option<HashMap<BlockHash, SidechainId>>,
        /// Check fork to child index
        fork_to_child: Option<HashMap<BlockHash, HashSet<BlockHash>>>,
        /// Pending blocks
        pending_blocks: Option<(BlockNumber, HashSet<BlockHash>)>,
        /// Buffered blocks
        buffered_blocks: Option<HashMap<BlockHash, SealedBlockWithSenders>>,
    }

    impl TreeTester {
        const fn with_chain_num(mut self, chain_num: usize) -> Self {
            self.chain_num = Some(chain_num);
            self
        }

        fn with_block_to_chain(mut self, block_to_chain: HashMap<BlockHash, SidechainId>) -> Self {
            self.block_to_chain = Some(block_to_chain);
            self
        }

        fn with_fork_to_child(
            mut self,
            fork_to_child: HashMap<BlockHash, HashSet<BlockHash>>,
        ) -> Self {
            self.fork_to_child = Some(fork_to_child);
            self
        }

        fn with_buffered_blocks(
            mut self,
            buffered_blocks: HashMap<BlockHash, SealedBlockWithSenders>,
        ) -> Self {
            self.buffered_blocks = Some(buffered_blocks);
            self
        }

        fn with_pending_blocks(
            mut self,
            pending_blocks: (BlockNumber, HashSet<BlockHash>),
        ) -> Self {
            self.pending_blocks = Some(pending_blocks);
            self
        }

        fn assert<N: NodeTypesWithDB, E: BlockExecutorProvider>(self, tree: &BlockchainTree<N, E>) {
            if let Some(chain_num) = self.chain_num {
                assert_eq!(tree.state.chains.len(), chain_num);
            }
            if let Some(block_to_chain) = self.block_to_chain {
                assert_eq!(*tree.state.block_indices.blocks_to_chain(), block_to_chain);
            }
            if let Some(fork_to_child) = self.fork_to_child {
                let mut x: HashMap<BlockHash, LinkedHashSet<BlockHash>> =
                    HashMap::with_capacity(fork_to_child.len());
                for (key, hash_set) in fork_to_child {
                    x.insert(key, hash_set.into_iter().collect());
                }
                assert_eq!(*tree.state.block_indices.fork_to_child(), x);
            }
            if let Some(pending_blocks) = self.pending_blocks {
                let (num, hashes) = tree.state.block_indices.pending_blocks();
                let hashes = hashes.into_iter().collect::<HashSet<_>>();
                assert_eq!((num, hashes), pending_blocks);
            }
            if let Some(buffered_blocks) = self.buffered_blocks {
                assert_eq!(*tree.state.buffered_blocks.blocks(), buffered_blocks);
            }
        }
    }

    #[test]
    fn consecutive_reorgs() {
        let signer = Address::random();
        let initial_signer_balance = U256::from(10).pow(U256::from(18));
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(Genesis {
                    alloc: BTreeMap::from([(
                        signer,
                        GenesisAccount { balance: initial_signer_balance, ..Default::default() },
                    )]),
                    ..MAINNET.genesis.clone()
                })
                .shanghai_activated()
                .build(),
        );
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        let consensus = Arc::new(TestConsensus::default());
        let executor_provider = EthExecutorProvider::ethereum(chain_spec.clone());

        {
            let provider_rw = provider_factory.provider_rw().unwrap();
            provider_rw
                .insert_block(
                    SealedBlock::new(chain_spec.sealed_genesis_header(), Default::default())
                        .try_seal_with_senders()
                        .unwrap(),
                )
                .unwrap();
            let account = Account { balance: initial_signer_balance, ..Default::default() };
            provider_rw.tx_ref().put::<tables::PlainAccountState>(signer, account).unwrap();
            provider_rw.tx_ref().put::<tables::HashedAccounts>(keccak256(signer), account).unwrap();
            provider_rw.commit().unwrap();
        }

        let single_tx_cost = U256::from(EIP1559_INITIAL_BASE_FEE * MIN_TRANSACTION_GAS);
        let mock_tx = |nonce: u64| -> TransactionSignedEcRecovered {
            TransactionSigned::from_transaction_and_signature(
                Transaction::Eip1559(TxEip1559 {
                    chain_id: chain_spec.chain.id(),
                    nonce,
                    gas_limit: MIN_TRANSACTION_GAS,
                    to: Address::ZERO.into(),
                    max_fee_per_gas: EIP1559_INITIAL_BASE_FEE as u128,
                    ..Default::default()
                }),
                Signature::test_signature(),
            )
            .with_signer(signer)
        };

        let mock_block = |number: u64,
                          parent: Option<B256>,
                          body: Vec<TransactionSignedEcRecovered>,
                          num_of_signer_txs: u64|
         -> SealedBlockWithSenders {
            let transactions_root = calculate_transaction_root(&body);
            let receipts = body
                .iter()
                .enumerate()
                .map(|(idx, tx)| {
                    Receipt {
                        tx_type: tx.tx_type(),
                        success: true,
                        cumulative_gas_used: (idx as u64 + 1) * MIN_TRANSACTION_GAS,
                        ..Default::default()
                    }
                    .with_bloom()
                })
                .collect::<Vec<_>>();

            // receipts root computation is different for OP
            let receipts_root = calculate_receipt_root(&receipts);

            let sealed = Header {
                number,
                parent_hash: parent.unwrap_or_default(),
                gas_used: body.len() as u64 * MIN_TRANSACTION_GAS,
                gas_limit: chain_spec.max_gas_limit,
                mix_hash: B256::random(),
                base_fee_per_gas: Some(EIP1559_INITIAL_BASE_FEE),
                transactions_root,
                receipts_root,
                state_root: state_root_unhashed(HashMap::from([(
                    signer,
                    (
                        AccountInfo {
                            balance: initial_signer_balance -
                                (single_tx_cost * U256::from(num_of_signer_txs)),
                            nonce: num_of_signer_txs,
                            ..Default::default()
                        },
                        EMPTY_ROOT_HASH,
                    ),
                )])),
                ..Default::default()
            }
            .seal_slow();
            let (header, seal) = sealed.into_parts();

            SealedBlockWithSenders::new(
                SealedBlock {
                    header: SealedHeader::new(header, seal),
                    body: BlockBody {
                        transactions: body.clone().into_iter().map(|tx| tx.into_signed()).collect(),
                        ommers: Vec::new(),
                        withdrawals: Some(Withdrawals::default()),
                    },
                },
                body.iter().map(|tx| tx.signer()).collect(),
            )
            .unwrap()
        };

        let fork_block = mock_block(1, Some(chain_spec.genesis_hash()), Vec::from([mock_tx(0)]), 1);

        let canonical_block_1 =
            mock_block(2, Some(fork_block.hash()), Vec::from([mock_tx(1), mock_tx(2)]), 3);
        let canonical_block_2 = mock_block(3, Some(canonical_block_1.hash()), Vec::new(), 3);
        let canonical_block_3 =
            mock_block(4, Some(canonical_block_2.hash()), Vec::from([mock_tx(3)]), 4);

        let sidechain_block_1 = mock_block(2, Some(fork_block.hash()), Vec::from([mock_tx(1)]), 2);
        let sidechain_block_2 =
            mock_block(3, Some(sidechain_block_1.hash()), Vec::from([mock_tx(2)]), 3);

        let mut tree = BlockchainTree::new(
            TreeExternals::new(provider_factory, consensus, executor_provider),
            BlockchainTreeConfig::default(),
        )
        .expect("failed to create tree");

        tree.insert_block(fork_block.clone(), BlockValidationKind::Exhaustive).unwrap();

        assert_eq!(
            tree.make_canonical(fork_block.hash()).unwrap(),
            CanonicalOutcome::Committed { head: fork_block.header.clone() }
        );

        assert_eq!(
            tree.insert_block(canonical_block_1.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        assert_eq!(
            tree.make_canonical(canonical_block_1.hash()).unwrap(),
            CanonicalOutcome::Committed { head: canonical_block_1.header.clone() }
        );

        assert_eq!(
            tree.insert_block(canonical_block_2, BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        assert_eq!(
            tree.insert_block(sidechain_block_1.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::HistoricalFork))
        );

        assert_eq!(
            tree.make_canonical(sidechain_block_1.hash()).unwrap(),
            CanonicalOutcome::Committed { head: sidechain_block_1.header.clone() }
        );

        assert_eq!(
            tree.make_canonical(canonical_block_1.hash()).unwrap(),
            CanonicalOutcome::Committed { head: canonical_block_1.header.clone() }
        );

        assert_eq!(
            tree.insert_block(sidechain_block_2.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::HistoricalFork))
        );

        assert_eq!(
            tree.make_canonical(sidechain_block_2.hash()).unwrap(),
            CanonicalOutcome::Committed { head: sidechain_block_2.header.clone() }
        );

        assert_eq!(
            tree.insert_block(canonical_block_3.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::HistoricalFork))
        );

        assert_eq!(
            tree.make_canonical(canonical_block_3.hash()).unwrap(),
            CanonicalOutcome::Committed { head: canonical_block_3.header.clone() }
        );
    }

    #[test]
    fn sidechain_block_hashes() {
        let data = BlockchainTestData::default_from_number(11);
        let (block1, exec1) = data.blocks[0].clone();
        let (block2, exec2) = data.blocks[1].clone();
        let (block3, exec3) = data.blocks[2].clone();
        let (block4, exec4) = data.blocks[3].clone();
        let genesis = data.genesis;

        // test pops execution results from vector, so order is from last to first.
        let externals =
            setup_externals(vec![exec3.clone(), exec2.clone(), exec4, exec3, exec2, exec1]);

        // last finalized block would be number 9.
        setup_genesis(&externals.provider_factory, genesis);

        // make tree
        let config = BlockchainTreeConfig::new(1, 2, 3, 2);
        let mut tree = BlockchainTree::new(externals, config).expect("failed to create tree");
        // genesis block 10 is already canonical
        tree.make_canonical(B256::ZERO).unwrap();

        // make genesis block 10 as finalized
        tree.finalize_block(10).unwrap();

        assert_eq!(
            tree.insert_block(block1.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        assert_eq!(
            tree.insert_block(block2.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        assert_eq!(
            tree.insert_block(block3.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        assert_eq!(
            tree.insert_block(block4, BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        let mut block2a = block2;
        let block2a_hash = B256::new([0x34; 32]);
        block2a.set_hash(block2a_hash);

        assert_eq!(
            tree.insert_block(block2a.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::HistoricalFork))
        );

        let mut block3a = block3;
        let block3a_hash = B256::new([0x35; 32]);
        block3a.set_hash(block3a_hash);
        block3a.set_parent_hash(block2a.hash());

        assert_eq!(
            tree.insert_block(block3a.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical)) /* TODO: this is incorrect, figure out why */
        );

        let block3a_chain_id = tree.state.block_indices.get_side_chain_id(&block3a.hash()).unwrap();
        assert_eq!(
            tree.all_chain_hashes(block3a_chain_id),
            BTreeMap::from([
                (block1.number, block1.hash()),
                (block2a.number, block2a.hash()),
                (block3a.number, block3a.hash()),
            ])
        );
    }

    #[test]
    fn cached_trie_updates() {
        let data = BlockchainTestData::default_from_number(11);
        let (block1, exec1) = data.blocks[0].clone();
        let (block2, exec2) = data.blocks[1].clone();
        let (block3, exec3) = data.blocks[2].clone();
        let (block4, exec4) = data.blocks[3].clone();
        let (block5, exec5) = data.blocks[4].clone();
        let genesis = data.genesis;

        // test pops execution results from vector, so order is from last to first.
        let externals = setup_externals(vec![exec5.clone(), exec4, exec3, exec2, exec1]);

        // last finalized block would be number 9.
        setup_genesis(&externals.provider_factory, genesis);

        // make tree
        let config = BlockchainTreeConfig::new(1, 2, 3, 2);
        let mut tree = BlockchainTree::new(externals, config).expect("failed to create tree");
        // genesis block 10 is already canonical
        tree.make_canonical(B256::ZERO).unwrap();

        // make genesis block 10 as finalized
        tree.finalize_block(10).unwrap();

        assert_eq!(
            tree.insert_block(block1.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );
        let block1_chain_id = tree.state.block_indices.get_side_chain_id(&block1.hash()).unwrap();
        let block1_chain = tree.state.chains.get(&block1_chain_id).unwrap();
        assert!(block1_chain.trie_updates().is_some());

        assert_eq!(
            tree.insert_block(block2.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );
        let block2_chain_id = tree.state.block_indices.get_side_chain_id(&block2.hash()).unwrap();
        let block2_chain = tree.state.chains.get(&block2_chain_id).unwrap();
        assert!(block2_chain.trie_updates().is_none());

        assert_eq!(
            tree.make_canonical(block2.hash()).unwrap(),
            CanonicalOutcome::Committed { head: block2.header.clone() }
        );

        assert_eq!(
            tree.insert_block(block3.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );
        let block3_chain_id = tree.state.block_indices.get_side_chain_id(&block3.hash()).unwrap();
        let block3_chain = tree.state.chains.get(&block3_chain_id).unwrap();
        assert!(block3_chain.trie_updates().is_some());

        assert_eq!(
            tree.make_canonical(block3.hash()).unwrap(),
            CanonicalOutcome::Committed { head: block3.header.clone() }
        );

        assert_eq!(
            tree.insert_block(block4.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );
        let block4_chain_id = tree.state.block_indices.get_side_chain_id(&block4.hash()).unwrap();
        let block4_chain = tree.state.chains.get(&block4_chain_id).unwrap();
        assert!(block4_chain.trie_updates().is_some());

        assert_eq!(
            tree.insert_block(block5.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        let block5_chain_id = tree.state.block_indices.get_side_chain_id(&block5.hash()).unwrap();
        let block5_chain = tree.state.chains.get(&block5_chain_id).unwrap();
        assert!(block5_chain.trie_updates().is_none());

        assert_eq!(
            tree.make_canonical(block5.hash()).unwrap(),
            CanonicalOutcome::Committed { head: block5.header.clone() }
        );

        let provider = tree.externals.provider_factory.provider().unwrap();
        let prefix_sets = exec5.hash_state_slow().construct_prefix_sets().freeze();
        let state_root =
            StateRoot::from_tx(provider.tx_ref()).with_prefix_sets(prefix_sets).root().unwrap();
        assert_eq!(state_root, block5.state_root);
    }

    #[test]
    fn test_side_chain_fork() {
        let data = BlockchainTestData::default_from_number(11);
        let (block1, exec1) = data.blocks[0].clone();
        let (block2, exec2) = data.blocks[1].clone();
        let genesis = data.genesis;

        // test pops execution results from vector, so order is from last to first.
        let externals = setup_externals(vec![exec2.clone(), exec2, exec1]);

        // last finalized block would be number 9.
        setup_genesis(&externals.provider_factory, genesis);

        // make tree
        let config = BlockchainTreeConfig::new(1, 2, 3, 2);
        let mut tree = BlockchainTree::new(externals, config).expect("failed to create tree");
        // genesis block 10 is already canonical
        tree.make_canonical(B256::ZERO).unwrap();

        // make genesis block 10 as finalized
        tree.finalize_block(10).unwrap();

        assert_eq!(
            tree.insert_block(block1.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        assert_eq!(
            tree.insert_block(block2.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        // we have one chain that has two blocks.
        // Trie state:
        //      b2 (pending block)
        //      |
        //      |
        //      b1 (pending block)
        //    /
        //  /
        // g1 (canonical blocks)
        // |
        TreeTester::default()
            .with_chain_num(1)
            .with_block_to_chain(HashMap::from([
                (block1.hash(), 0.into()),
                (block2.hash(), 0.into()),
            ]))
            .with_fork_to_child(HashMap::from([(
                block1.parent_hash,
                HashSet::from([block1.hash()]),
            )]))
            .assert(&tree);

        let mut block2a = block2.clone();
        let block2a_hash = B256::new([0x34; 32]);
        block2a.set_hash(block2a_hash);

        assert_eq!(
            tree.insert_block(block2a.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::HistoricalFork))
        );

        // fork chain.
        // Trie state:
        //      b2  b2a (pending blocks in tree)
        //      |   /
        //      | /
        //      b1
        //    /
        //  /
        // g1 (canonical blocks)
        // |

        TreeTester::default()
            .with_chain_num(2)
            .with_block_to_chain(HashMap::from([
                (block1.hash(), 0.into()),
                (block2.hash(), 0.into()),
                (block2a.hash(), 1.into()),
            ]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1.hash()])),
                (block2a.parent_hash, HashSet::from([block2a.hash()])),
            ]))
            .assert(&tree);
        // chain 0 has two blocks so receipts and reverts len is 2
        let chain0 = tree.state.chains.get(&0.into()).unwrap().execution_outcome();
        assert_eq!(chain0.receipts().len(), 2);
        assert_eq!(chain0.state().reverts.len(), 2);
        assert_eq!(chain0.first_block(), block1.number);
        // chain 1 has one block so receipts and reverts len is 1
        let chain1 = tree.state.chains.get(&1.into()).unwrap().execution_outcome();
        assert_eq!(chain1.receipts().len(), 1);
        assert_eq!(chain1.state().reverts.len(), 1);
        assert_eq!(chain1.first_block(), block2.number);
    }

    #[test]
    fn sanity_path() {
        let data = BlockchainTestData::default_from_number(11);
        let (block1, exec1) = data.blocks[0].clone();
        let (block2, exec2) = data.blocks[1].clone();
        let genesis = data.genesis;

        // test pops execution results from vector, so order is from last to first.
        let externals = setup_externals(vec![exec2.clone(), exec1.clone(), exec2, exec1]);

        // last finalized block would be number 9.
        setup_genesis(&externals.provider_factory, genesis);

        // make tree
        let config = BlockchainTreeConfig::new(1, 2, 3, 2);
        let mut tree = BlockchainTree::new(externals, config).expect("failed to create tree");

        let mut canon_notif = tree.subscribe_canon_state();
        // genesis block 10 is already canonical
        let head = BlockNumHash::new(10, B256::ZERO);
        tree.make_canonical(head.hash).unwrap();

        // make sure is_block_hash_canonical returns true for genesis block
        tree.is_block_hash_canonical(&B256::ZERO).unwrap();

        // make genesis block 10 as finalized
        tree.finalize_block(head.number).unwrap();

        // block 2 parent is not known, block2 is buffered.
        assert_eq!(
            tree.insert_block(block2.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Disconnected {
                head,
                missing_ancestor: block2.parent_num_hash()
            })
        );

        // Buffered block: [block2]
        // Trie state:
        // |
        // g1 (canonical blocks)
        // |

        TreeTester::default()
            .with_buffered_blocks(HashMap::from([(block2.hash(), block2.clone())]))
            .assert(&tree);

        assert_eq!(
            tree.is_block_known(block2.num_hash()).unwrap(),
            Some(BlockStatus::Disconnected { head, missing_ancestor: block2.parent_num_hash() })
        );

        // check if random block is known
        let old_block = BlockNumHash::new(1, B256::new([32; 32]));
        let err = BlockchainTreeError::PendingBlockIsFinalized { last_finalized: 10 };

        assert_eq!(tree.is_block_known(old_block).unwrap_err().as_tree_error(), Some(err));

        // insert block1 and buffered block2 is inserted
        assert_eq!(
            tree.insert_block(block1.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        // Buffered blocks: []
        // Trie state:
        //      b2 (pending block)
        //      |
        //      |
        //      b1 (pending block)
        //    /
        //  /
        // g1 (canonical blocks)
        // |
        TreeTester::default()
            .with_chain_num(1)
            .with_block_to_chain(HashMap::from([
                (block1.hash(), 0.into()),
                (block2.hash(), 0.into()),
            ]))
            .with_fork_to_child(HashMap::from([(
                block1.parent_hash,
                HashSet::from([block1.hash()]),
            )]))
            .with_pending_blocks((block1.number, HashSet::from([block1.hash()])))
            .assert(&tree);

        // already inserted block will `InsertPayloadOk::AlreadySeen(_)`
        assert_eq!(
            tree.insert_block(block1.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::AlreadySeen(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        // block two is already inserted.
        assert_eq!(
            tree.insert_block(block2.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::AlreadySeen(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        // make block1 canonical
        tree.make_canonical(block1.hash()).unwrap();
        // check notification
        assert_matches!(canon_notif.try_recv(), Ok(CanonStateNotification::Commit{ new}) if *new.blocks() == BTreeMap::from([(block1.number,block1.clone())]));

        // make block2 canonicals
        tree.make_canonical(block2.hash()).unwrap();
        // check notification.
        assert_matches!(canon_notif.try_recv(), Ok(CanonStateNotification::Commit{ new}) if *new.blocks() == BTreeMap::from([(block2.number,block2.clone())]));

        // Trie state:
        // b2 (canonical block)
        // |
        // |
        // b1 (canonical block)
        // |
        // |
        // g1 (canonical blocks)
        // |
        TreeTester::default()
            .with_chain_num(0)
            .with_block_to_chain(HashMap::from([]))
            .with_fork_to_child(HashMap::from([]))
            .assert(&tree);

        /**** INSERT SIDE BLOCKS *** */

        let mut block1a = block1.clone();
        let block1a_hash = B256::new([0x33; 32]);
        block1a.set_hash(block1a_hash);
        let mut block2a = block2.clone();
        let block2a_hash = B256::new([0x34; 32]);
        block2a.set_hash(block2a_hash);

        // reinsert two blocks that point to canonical chain
        assert_eq!(
            tree.insert_block(block1a.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::HistoricalFork))
        );

        TreeTester::default()
            .with_chain_num(1)
            .with_block_to_chain(HashMap::from([(block1a_hash, 1.into())]))
            .with_fork_to_child(HashMap::from([(
                block1.parent_hash,
                HashSet::from([block1a_hash]),
            )]))
            .with_pending_blocks((block2.number + 1, HashSet::from([])))
            .assert(&tree);

        assert_eq!(
            tree.insert_block(block2a.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::HistoricalFork))
        );
        // Trie state:
        // b2   b2a (side chain)
        // |   /
        // | /
        // b1  b1a (side chain)
        // |  /
        // |/
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(2)
            .with_block_to_chain(HashMap::from([
                (block1a_hash, 1.into()),
                (block2a_hash, 2.into()),
            ]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1a_hash])),
                (block1.hash(), HashSet::from([block2a_hash])),
            ]))
            .with_pending_blocks((block2.number + 1, HashSet::from([])))
            .assert(&tree);

        // make b2a canonical
        assert!(tree.make_canonical(block2a_hash).is_ok());
        // check notification.
        assert_matches!(canon_notif.try_recv(),
            Ok(CanonStateNotification::Reorg{ old, new})
            if *old.blocks() == BTreeMap::from([(block2.number,block2.clone())])
                && *new.blocks() == BTreeMap::from([(block2a.number,block2a.clone())]));

        // Trie state:
        // b2a   b2 (side chain)
        // |   /
        // | /
        // b1  b1a (side chain)
        // |  /
        // |/
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(2)
            .with_block_to_chain(HashMap::from([
                (block1a_hash, 1.into()),
                (block2.hash(), 3.into()),
            ]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1a_hash])),
                (block1.hash(), HashSet::from([block2.hash()])),
            ]))
            .with_pending_blocks((block2.number + 1, HashSet::default()))
            .assert(&tree);

        assert_matches!(tree.make_canonical(block1a_hash), Ok(_));
        // Trie state:
        //       b2a   b2 (side chain)
        //       |   /
        //       | /
        // b1a  b1 (side chain)
        // |  /
        // |/
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(2)
            .with_block_to_chain(HashMap::from([
                (block1.hash(), 4.into()),
                (block2a_hash, 4.into()),
                (block2.hash(), 3.into()),
            ]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1.hash()])),
                (block1.hash(), HashSet::from([block2.hash()])),
            ]))
            .with_pending_blocks((block1a.number + 1, HashSet::default()))
            .assert(&tree);

        // check notification.
        assert_matches!(canon_notif.try_recv(),
            Ok(CanonStateNotification::Reorg{ old, new})
            if *old.blocks() == BTreeMap::from([(block1.number,block1.clone()),(block2a.number,block2a.clone())])
                && *new.blocks() == BTreeMap::from([(block1a.number,block1a.clone())]));

        // check that b2 and b1 are not canonical
        assert!(!tree.is_block_hash_canonical(&block2.hash()).unwrap());
        assert!(!tree.is_block_hash_canonical(&block1.hash()).unwrap());

        // ensure that b1a is canonical
        assert!(tree.is_block_hash_canonical(&block1a.hash()).unwrap());

        // make b2 canonical
        tree.make_canonical(block2.hash()).unwrap();
        // Trie state:
        // b2   b2a (side chain)
        // |   /
        // | /
        // b1  b1a (side chain)
        // |  /
        // |/
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(2)
            .with_block_to_chain(HashMap::from([
                (block1a_hash, 5.into()),
                (block2a_hash, 4.into()),
            ]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1a_hash])),
                (block1.hash(), HashSet::from([block2a_hash])),
            ]))
            .with_pending_blocks((block2.number + 1, HashSet::default()))
            .assert(&tree);

        // check notification.
        assert_matches!(canon_notif.try_recv(),
            Ok(CanonStateNotification::Reorg{ old, new})
            if *old.blocks() == BTreeMap::from([(block1a.number,block1a.clone())])
                && *new.blocks() == BTreeMap::from([(block1.number,block1.clone()),(block2.number,block2.clone())]));

        // check that b2 is now canonical
        assert!(tree.is_block_hash_canonical(&block2.hash()).unwrap());

        // finalize b1 that would make b1a removed from tree
        tree.finalize_block(11).unwrap();
        // Trie state:
        // b2   b2a (side chain)
        // |   /
        // | /
        // b1 (canon)
        // |
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(1)
            .with_block_to_chain(HashMap::from([(block2a_hash, 4.into())]))
            .with_fork_to_child(HashMap::from([(block1.hash(), HashSet::from([block2a_hash]))]))
            .with_pending_blocks((block2.number + 1, HashSet::from([])))
            .assert(&tree);

        // unwind canonical
        assert!(tree.unwind(block1.number).is_ok());
        // Trie state:
        //    b2   b2a (pending block)
        //   /    /
        //  /   /
        // /  /
        // b1 (canonical block)
        // |
        // |
        // g1 (canonical blocks)
        // |
        TreeTester::default()
            .with_chain_num(2)
            .with_block_to_chain(HashMap::from([
                (block2a_hash, 4.into()),
                (block2.hash(), 6.into()),
            ]))
            .with_fork_to_child(HashMap::from([(
                block1.hash(),
                HashSet::from([block2a_hash, block2.hash()]),
            )]))
            .with_pending_blocks((block2.number, HashSet::from([block2.hash(), block2a.hash()])))
            .assert(&tree);

        // commit b2a
        tree.make_canonical(block2.hash()).unwrap();

        // Trie state:
        // b2   b2a (side chain)
        // |   /
        // | /
        // b1 (finalized)
        // |
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(1)
            .with_block_to_chain(HashMap::from([(block2a_hash, 4.into())]))
            .with_fork_to_child(HashMap::from([(block1.hash(), HashSet::from([block2a_hash]))]))
            .with_pending_blocks((block2.number + 1, HashSet::default()))
            .assert(&tree);

        // check notification.
        assert_matches!(canon_notif.try_recv(),
            Ok(CanonStateNotification::Commit{ new })
            if *new.blocks() == BTreeMap::from([(block2.number,block2.clone())]));

        // insert unconnected block2b
        let mut block2b = block2a.clone();
        block2b.set_hash(B256::new([0x99; 32]));
        block2b.set_parent_hash(B256::new([0x88; 32]));

        assert_eq!(
            tree.insert_block(block2b.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Disconnected {
                head: block2.header.num_hash(),
                missing_ancestor: block2b.parent_num_hash()
            })
        );

        TreeTester::default()
            .with_buffered_blocks(HashMap::from([(block2b.hash(), block2b.clone())]))
            .assert(&tree);

        // update canonical block to b2, this would make b2a be removed
        assert!(tree.connect_buffered_blocks_to_canonical_hashes_and_finalize(12).is_ok());

        assert_eq!(
            tree.is_block_known(block2.num_hash()).unwrap(),
            Some(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        // Trie state:
        // b2 (finalized)
        // |
        // b1 (finalized)
        // |
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(0)
            .with_block_to_chain(HashMap::default())
            .with_fork_to_child(HashMap::default())
            .with_pending_blocks((block2.number + 1, HashSet::default()))
            .with_buffered_blocks(HashMap::default())
            .assert(&tree);
    }

    #[test]
    fn last_finalized_block_initialization() {
        let data = BlockchainTestData::default_from_number(11);
        let (block1, exec1) = data.blocks[0].clone();
        let (block2, exec2) = data.blocks[1].clone();
        let (block3, exec3) = data.blocks[2].clone();
        let genesis = data.genesis;

        // test pops execution results from vector, so order is from last to first.
        let externals =
            setup_externals(vec![exec3.clone(), exec2.clone(), exec1.clone(), exec3, exec2, exec1]);
        let cloned_externals_1 = TreeExternals {
            provider_factory: externals.provider_factory.clone(),
            executor_factory: externals.executor_factory.clone(),
            consensus: externals.consensus.clone(),
        };
        let cloned_externals_2 = TreeExternals {
            provider_factory: externals.provider_factory.clone(),
            executor_factory: externals.executor_factory.clone(),
            consensus: externals.consensus.clone(),
        };

        // last finalized block would be number 9.
        setup_genesis(&externals.provider_factory, genesis);

        // make tree
        let config = BlockchainTreeConfig::new(1, 2, 3, 2);
        let mut tree = BlockchainTree::new(externals, config).expect("failed to create tree");

        assert_eq!(
            tree.insert_block(block1.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        assert_eq!(
            tree.insert_block(block2.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        assert_eq!(
            tree.insert_block(block3, BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::Canonical))
        );

        tree.make_canonical(block2.hash()).unwrap();

        // restart
        let mut tree =
            BlockchainTree::new(cloned_externals_1, config).expect("failed to create tree");
        assert_eq!(tree.block_indices().last_finalized_block(), 0);

        let mut block1a = block1;
        let block1a_hash = B256::new([0x33; 32]);
        block1a.set_hash(block1a_hash);

        assert_eq!(
            tree.insert_block(block1a.clone(), BlockValidationKind::Exhaustive).unwrap(),
            InsertPayloadOk::Inserted(BlockStatus::Valid(BlockAttachment::HistoricalFork))
        );

        tree.make_canonical(block1a.hash()).unwrap();
        tree.finalize_block(block1a.number).unwrap();

        // restart
        let tree = BlockchainTree::new(cloned_externals_2, config).expect("failed to create tree");

        assert_eq!(tree.block_indices().last_finalized_block(), block1a.number);
    }
}
