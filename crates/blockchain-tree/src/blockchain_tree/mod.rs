//! Implementation of [`BlockchainTree`]
use chain::BlockChainId;
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::{
    blockchain_tree::BlockStatus, consensus::Consensus, executor::Error as ExecError, Error,
};
use reth_primitives::{
    BlockHash, BlockNumber, ForkBlock, Hardfork, SealedBlock, SealedBlockWithSenders, U256,
};
use reth_provider::{
    chain::{ChainSplit, SplitAt},
    post_state::PostState,
    CanonStateNotification, CanonStateNotificationSender, CanonStateNotifications, Chain,
    ExecutorFactory, HeaderProvider, Transaction,
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

pub mod block_indices;
use block_indices::BlockIndices;

pub mod chain;
pub use chain::AppendableChain;

pub mod config;
use config::BlockchainTreeConfig;

pub mod externals;
use externals::TreeExternals;

pub mod shareable;
pub use shareable::ShareableBlockchainTree;

pub mod post_state_data;
pub use post_state_data::{PostStateData, PostStateDataRef};

#[cfg_attr(doc, aquamarine::aquamarine)]
/// Tree of chains and its identifications.
///
/// Mermaid flowchart represent all blocks that can appear in blockchain.
/// Green blocks belong to canonical chain and are saved inside database table, they are our main
/// chain. Pending blocks and sidechains are found in memory inside [`BlockchainTree`].
/// Both pending and sidechains have same mechanisms only difference is when they got committed to
/// database. For pending it is just append operation but for sidechains they need to move current
/// canonical blocks to BlockchainTree flush sidechain to the database to become canonical chain.
/// ```mermaid
/// flowchart BT
/// subgraph canonical chain
/// CanonState:::state
/// block0canon:::canon -->block1canon:::canon -->block2canon:::canon -->block3canon:::canon --> block4canon:::canon --> block5canon:::canon
/// end
/// block5canon --> block6pending1:::pending
/// block5canon --> block6pending2:::pending
/// subgraph sidechain2
/// S2State:::state
/// block3canon --> block4s2:::sidechain --> block5s2:::sidechain
/// end
/// subgraph sidechain1
/// S1State:::state
/// block2canon --> block3s1:::sidechain --> block4s1:::sidechain --> block5s1:::sidechain --> block6s1:::sidechain
/// end
/// classDef state fill:#1882C4
/// classDef canon fill:#8AC926
/// classDef pending fill:#FFCA3A
/// classDef sidechain fill:#FF595E
/// ```
///
///
/// main functions:
/// * [BlockchainTree::insert_block]: Connect block to chain, execute it and if valid insert block
///   inside tree.
/// * [BlockchainTree::finalize_block]: Remove chains that join to now finalized block, as chain
///   becomes invalid.
/// * [BlockchainTree::make_canonical]: Check if we have the hash of block that we want to finalize
///   and commit it to db. If we don't have the block, pipeline syncing should start to fetch the
///   blocks from p2p. Do reorg in tables if canonical chain if needed.
#[derive(Debug)]
pub struct BlockchainTree<DB: Database, C: Consensus, EF: ExecutorFactory> {
    /// The tracked chains and their current data.
    chains: HashMap<BlockChainId, AppendableChain>,
    /// Static blockchain ID generator
    block_chain_id_generator: u64,
    /// Indices to block and their connection to the canonical chain.
    block_indices: BlockIndices,
    /// External components (the database, consensus engine etc.)
    externals: TreeExternals<DB, C, EF>,
    /// Tree configuration
    config: BlockchainTreeConfig,
    /// Broadcast channel for canon state changes notifications.
    canon_state_notification_sender: CanonStateNotificationSender,
}

/// A container that wraps chains and block indices to allow searching for block hashes across all
/// sidechains.
pub struct BlockHashes<'a> {
    /// The current tracked chains.
    pub chains: &'a mut HashMap<BlockChainId, AppendableChain>,
    /// The block indices for all chains.
    pub indices: &'a BlockIndices,
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTree<DB, C, EF> {
    /// Create a new blockchain tree.
    pub fn new(
        externals: TreeExternals<DB, C, EF>,
        canon_state_notification_sender: CanonStateNotificationSender,
        config: BlockchainTreeConfig,
    ) -> Result<Self, Error> {
        let max_reorg_depth = config.max_reorg_depth();

        let last_canonical_hashes = externals
            .db
            .tx()?
            .cursor_read::<tables::CanonicalHeaders>()?
            .walk_back(None)?
            .take((max_reorg_depth + config.num_of_additional_canonical_block_hashes()) as usize)
            .collect::<Result<Vec<(BlockNumber, BlockHash)>, _>>()?;

        // TODO(rakita) save last finalized block inside database but for now just take
        // tip-max_reorg_depth
        // task: https://github.com/paradigmxyz/reth/issues/1712
        let (last_finalized_block_number, _) =
            if last_canonical_hashes.len() > max_reorg_depth as usize {
                last_canonical_hashes[max_reorg_depth as usize]
            } else {
                // it is in reverse order from tip to N
                last_canonical_hashes.last().cloned().unwrap_or_default()
            };

        Ok(Self {
            externals,
            block_chain_id_generator: 0,
            chains: Default::default(),
            block_indices: BlockIndices::new(
                last_finalized_block_number,
                BTreeMap::from_iter(last_canonical_hashes.into_iter()),
            ),
            config,
            canon_state_notification_sender,
        })
    }

    /// Expose internal indices of the BlockchainTree.
    pub fn block_indices(&self) -> &BlockIndices {
        &self.block_indices
    }

    /// Returns the block with matching hash.
    pub fn block_by_hash(&self, block_hash: BlockHash) -> Option<&SealedBlock> {
        let id = self.block_indices.get_blocks_chain_id(&block_hash)?;
        let chain = self.chains.get(&id)?;
        chain.block(block_hash)
    }

    /// Return items needed to execute on the pending state.
    /// This includes:
    ///     * `BlockHash` of canonical block that chain connects to. Needed for creating database
    ///       provider for the rest of the state.
    ///     * `PostState` changes that happened at the asked `block_hash`
    ///     * `BTreeMap<BlockNumber,BlockHash>` list of past pending and canonical hashes, That are
    ///       needed for evm `BLOCKHASH` opcode.
    /// Return none if block is not known.
    pub fn post_state_data(&self, block_hash: BlockHash) -> Option<PostStateData> {
        // if it is part of the chain
        if let Some(chain_id) = self.block_indices.get_blocks_chain_id(&block_hash) {
            // get block state
            let chain = self.chains.get(&chain_id).expect("Chain should be present");
            let block_number = chain.block_number(block_hash)?;
            let state = chain.state_at_block(block_number)?;

            // get parent hashes
            let mut parent_block_hashed = self.all_chain_hashes(chain_id);
            let first_pending_block_number =
                *parent_block_hashed.first_key_value().expect("There is at least one block hash").0;
            let canonical_chain = self
                .block_indices
                .canonical_chain()
                .clone()
                .into_iter()
                .filter(|&(key, _)| key < first_pending_block_number)
                .collect::<Vec<_>>();
            parent_block_hashed.extend(canonical_chain.into_iter());

            // get canonical fork.
            let canonical_fork = self.canonical_fork(chain_id)?;
            return Some(PostStateData { state, parent_block_hashed, canonical_fork })
        }

        // check if there is canonical block
        if let Some(canonical_fork) =
            self.block_indices().canonical_chain().iter().find(|(_, value)| **value == block_hash)
        {
            return Some(PostStateData {
                canonical_fork: ForkBlock { number: *canonical_fork.0, hash: *canonical_fork.1 },
                state: PostState::new(),
                parent_block_hashed: self.block_indices().canonical_chain().clone(),
            })
        }

        None
    }

    /// Try inserting block inside the tree.
    /// If blocks does not have parent [`BlockStatus::Disconnected`] would be returned
    pub fn try_insert_block(
        &mut self,
        block: SealedBlockWithSenders,
    ) -> Result<BlockStatus, Error> {
        // check if block parent can be found in Tree

        // Create a new sidechain by forking the given chain, or append the block if the parent
        // block is the top of the given chain.
        if let Some(chain_id) = self.block_indices.get_blocks_chain_id(&block.parent_hash) {
            let block_hashes = self.all_chain_hashes(chain_id);

            // get canonical fork.
            let canonical_fork = self
                .canonical_fork(chain_id)
                .ok_or(ExecError::BlockChainIdConsistency { chain_id })?;

            // get chain that block needs to join to.
            let parent_chain = self
                .chains
                .get_mut(&chain_id)
                .ok_or(ExecError::BlockChainIdConsistency { chain_id })?;
            let chain_tip = parent_chain.tip().hash();

            let canonical_block_hashes = self.block_indices.canonical_chain();

            // append the block if it is continuing the chain.
            if chain_tip == block.parent_hash {
                let block_hash = block.hash();
                let block_number = block.number;
                parent_chain.append_block(
                    block,
                    block_hashes,
                    canonical_block_hashes,
                    canonical_fork,
                    &self.externals,
                )?;

                self.block_indices.insert_non_fork_block(block_number, block_hash, chain_id);
                return Ok(BlockStatus::Valid)
            } else {
                let chain = parent_chain.new_chain_fork(
                    block,
                    block_hashes,
                    canonical_block_hashes,
                    canonical_fork,
                    &self.externals,
                )?;
                self.insert_chain(chain);
                return Ok(BlockStatus::Accepted)
            }
        }
        // if not found, check if the parent can be found inside canonical chain.
        if Some(block.parent_hash) == self.block_indices.canonical_hash(&(block.number - 1)) {
            // create new chain that points to that block
            //return self.fork_canonical_chain(block.clone());
            // TODO save pending block to database
            // https://github.com/paradigmxyz/reth/issues/1713

            let db = self.externals.shareable_db();
            let fork_block = ForkBlock { number: block.number - 1, hash: block.parent_hash };

            // Validate that the block is post merge
            let parent_td = db
                .header_td(&block.parent_hash)?
                .ok_or(ExecError::CanonicalChain { block_hash: block.parent_hash })?;
            // Pass the parent total difficulty to short-circuit unnecessary calculations.
            if !self.externals.chain_spec.fork(Hardfork::Paris).active_at_ttd(parent_td, U256::ZERO)
            {
                return Err(ExecError::BlockPreMerge { hash: block.hash }.into())
            }

            // Create state provider
            let canonical_block_hashes = self.block_indices.canonical_chain();
            let canonical_tip =
                canonical_block_hashes.last_key_value().map(|(_, hash)| *hash).unwrap_or_default();
            let block_status = if block.parent_hash == canonical_tip {
                BlockStatus::Valid
            } else {
                BlockStatus::Accepted
            };

            let parent_header = db
                .header(&block.parent_hash)?
                .ok_or(ExecError::CanonicalChain { block_hash: block.parent_hash })?
                .seal(block.parent_hash);
            let chain = AppendableChain::new_canonical_fork(
                &block,
                &parent_header,
                canonical_block_hashes,
                fork_block,
                &self.externals,
            )?;
            self.insert_chain(chain);
            return Ok(block_status)
        }

        // NOTE: Block doesn't have a parent, and if we receive this block in `make_canonical`
        // function this could be a trigger to initiate p2p syncing, as we are missing the
        // parent.
        Ok(BlockStatus::Disconnected)
    }

    /// Get all block hashes from a sidechain that are not part of the canonical chain.
    ///
    /// This is a one time operation per block.
    ///
    /// # Note
    ///
    /// This is not cached in order to save memory.
    fn all_chain_hashes(&self, chain_id: BlockChainId) -> BTreeMap<BlockNumber, BlockHash> {
        // find chain and iterate over it,
        let mut chain_id = chain_id;
        let mut hashes = BTreeMap::new();
        loop {
            let Some(chain) = self.chains.get(&chain_id) else { return hashes };
            hashes.extend(chain.blocks().values().map(|b| (b.number, b.hash())));

            let fork_block = chain.fork_block_hash();
            if let Some(next_chain_id) = self.block_indices.get_blocks_chain_id(&fork_block) {
                chain_id = next_chain_id;
            } else {
                // if there is no fork block that point to other chains, break the loop.
                // it means that this fork joins to canonical block.
                break
            }
        }
        hashes
    }

    /// Get the block at which the given chain forked from the current canonical chain.
    ///
    /// This is used to figure out what kind of state provider the executor should use to execute
    /// the block.
    ///
    /// Returns `None` if the chain is not known.
    fn canonical_fork(&self, chain_id: BlockChainId) -> Option<ForkBlock> {
        let mut chain_id = chain_id;
        let mut fork;
        loop {
            // chain fork block
            fork = self.chains.get(&chain_id)?.fork_block();
            // get fork block chain
            if let Some(fork_chain_id) = self.block_indices.get_blocks_chain_id(&fork.hash) {
                chain_id = fork_chain_id;
                continue
            }
            break
        }
        (self.block_indices.canonical_hash(&fork.number) == Some(fork.hash)).then_some(fork)
    }

    /// Insert a chain into the tree.
    ///
    /// Inserts a chain into the tree and builds the block indices.
    fn insert_chain(&mut self, chain: AppendableChain) -> BlockChainId {
        let chain_id = self.block_chain_id_generator;
        self.block_chain_id_generator += 1;
        self.block_indices.insert_chain(chain_id, &chain);
        // add chain_id -> chain index
        self.chains.insert(chain_id, chain);
        chain_id
    }

    /// Insert a new block in the tree.
    ///
    /// # Note
    ///
    /// This recovers transaction signers (unlike [`BlockchainTree::insert_block_with_senders`]).
    pub fn insert_block(&mut self, block: SealedBlock) -> Result<BlockStatus, Error> {
        let block = block.seal_with_senders().ok_or(ExecError::SenderRecoveryError)?;
        self.insert_block_with_senders(block)
    }

    /// Insert a block (with senders recovered) in the tree.
    ///
    /// Returns `true` if:
    ///
    /// - The block is already part of a sidechain in the tree, or
    /// - The block is already part of the canonical chain, or
    /// - The parent is part of a sidechain in the tree, and we can fork at this block, or
    /// - The parent is part of the canonical chain, and we can fork at this block
    ///
    /// Otherwise `false` is returned, indicating that neither the block nor its parent is part of
    /// the chain or any sidechains.
    ///
    /// This means that if the block becomes canonical, we need to fetch the missing blocks over
    /// P2P.
    ///
    /// # Note
    ///
    /// If the senders have not already been recovered, call [`BlockchainTree::insert_block`]
    /// instead.
    pub fn insert_block_with_senders(
        &mut self,
        block: SealedBlockWithSenders,
    ) -> Result<BlockStatus, Error> {
        // check if block number is inside pending block slide
        let last_finalized_block = self.block_indices.last_finalized_block();
        if block.number <= last_finalized_block {
            return Err(ExecError::PendingBlockIsFinalized {
                block_number: block.number,
                block_hash: block.hash(),
                last_finalized: last_finalized_block,
            }
            .into())
        }

        // we will not even try to insert blocks that are too far in future.
        if block.number > last_finalized_block + self.config.max_blocks_in_chain() {
            return Err(ExecError::PendingBlockIsInFuture {
                block_number: block.number,
                block_hash: block.hash(),
                last_finalized: last_finalized_block,
            }
            .into())
        }

        // check if block known and is already inside Tree
        if let Some(chain_id) = self.block_indices.get_blocks_chain_id(&block.hash()) {
            let canonical_fork = self.canonical_fork(chain_id).expect("Chain id is valid");
            // if block chain extends canonical chain
            if canonical_fork == self.block_indices.canonical_tip() {
                return Ok(BlockStatus::Valid)
            } else {
                return Ok(BlockStatus::Accepted)
            }
        }

        // check if block is part of canonical chain
        if self.block_indices.canonical_hash(&block.number) == Some(block.hash()) {
            // block is part of canonical chain
            return Ok(BlockStatus::Valid)
        }

        self.try_insert_block(block)
    }

    /// Finalize blocks up until and including `finalized_block`, and remove them from the tree.
    pub fn finalize_block(&mut self, finalized_block: BlockNumber) {
        let mut remove_chains = self.block_indices.finalize_canonical_blocks(
            finalized_block,
            self.config.num_of_additional_canonical_block_hashes(),
        );

        while let Some(chain_id) = remove_chains.pop_first() {
            if let Some(chain) = self.chains.remove(&chain_id) {
                remove_chains.extend(self.block_indices.remove_chain(&chain));
            }
        }
    }

    /// Reads the last `N` canonical hashes from the database and updates the block indices of the
    /// tree.
    ///
    /// `N` is the `max_reorg_depth` plus the number of block hashes needed to satisfy the
    /// `BLOCKHASH` opcode in the EVM.
    ///
    /// # Note
    ///
    /// This finalizes `last_finalized_block` prior to reading the canonical hashes (using
    /// [`BlockchainTree::finalize_block`]).
    pub fn restore_canonical_hashes(
        &mut self,
        last_finalized_block: BlockNumber,
    ) -> Result<(), Error> {
        self.finalize_block(last_finalized_block);

        let num_of_canonical_hashes =
            self.config.max_reorg_depth() + self.config.num_of_additional_canonical_block_hashes();

        let last_canonical_hashes = self
            .externals
            .db
            .tx()?
            .cursor_read::<tables::CanonicalHeaders>()?
            .walk_back(None)?
            .take(num_of_canonical_hashes as usize)
            .collect::<Result<BTreeMap<BlockNumber, BlockHash>, _>>()?;

        let mut remove_chains = self.block_indices.update_block_hashes(last_canonical_hashes);

        // remove all chains that got discarded
        while let Some(chain_id) = remove_chains.first() {
            if let Some(chain) = self.chains.remove(chain_id) {
                remove_chains.extend(self.block_indices.remove_chain(&chain));
            }
        }

        Ok(())
    }

    /// Split a sidechain at the given point, and return the canonical part of it.
    ///
    /// The pending part of the chain is reinserted into the tree with the same `chain_id`.
    fn split_chain(
        &mut self,
        chain_id: BlockChainId,
        chain: AppendableChain,
        split_at: SplitAt,
    ) -> Chain {
        let chain = chain.into_inner();
        match chain.split(split_at) {
            ChainSplit::Split { canonical, pending } => {
                // rest of splited chain is inserted back with same chain_id.
                self.block_indices.insert_chain(chain_id, &pending);
                self.chains.insert(chain_id, AppendableChain::new(pending));
                canonical
            }
            ChainSplit::NoSplitCanonical(canonical) => canonical,
            ChainSplit::NoSplitPending(_) => {
                panic!("Should not happen as block indices guarantee structure of blocks")
            }
        }
    }

    /// Make a block and its parent part of the canonical chain.
    ///
    /// # Note
    ///
    /// This unwinds the database if necessary, i.e. if parts of the canonical chain have been
    /// re-orged.
    ///
    /// # Returns
    ///
    /// Returns `Ok` if the blocks were canonicalized, or if the blocks were already canonical.
    pub fn make_canonical(&mut self, block_hash: &BlockHash) -> Result<(), Error> {
        // If block is already canonical don't return error.
        if self.block_indices.is_block_hash_canonical(block_hash) {
            let td = self
                .externals
                .shareable_db()
                .header_td(block_hash)?
                .ok_or(ExecError::MissingTotalDifficulty { hash: *block_hash })?;
            if !self.externals.chain_spec.fork(Hardfork::Paris).active_at_ttd(td, U256::ZERO) {
                return Err(ExecError::BlockPreMerge { hash: *block_hash }.into())
            }
            return Ok(())
        }

        let Some(chain_id) = self.block_indices.get_blocks_chain_id(block_hash) else {
            return Err(ExecError::BlockHashNotFoundInChain { block_hash: *block_hash }.into())
        };
        let chain = self.chains.remove(&chain_id).expect("To be present");

        // we are spliting chain as there is possibility that only part of chain get canonicalized.
        let canonical = self.split_chain(chain_id, chain, SplitAt::Hash(*block_hash));

        let mut block_fork = canonical.fork_block();
        let mut block_fork_number = canonical.fork_block_number();
        let mut chains_to_promote = vec![canonical];

        // loop while fork blocks are found in Tree.
        while let Some(chain_id) = self.block_indices.get_blocks_chain_id(&block_fork.hash) {
            let chain = self.chains.remove(&chain_id).expect("To fork to be present");
            block_fork = chain.fork_block();
            let canonical = self.split_chain(chain_id, chain, SplitAt::Number(block_fork_number));
            block_fork_number = canonical.fork_block_number();
            chains_to_promote.push(canonical);
        }

        let old_tip = self.block_indices.canonical_tip();
        // Merge all chain into one chain.
        let mut new_canon_chain = chains_to_promote.pop().expect("There is at least one block");
        for chain in chains_to_promote.into_iter().rev() {
            new_canon_chain.append_chain(chain).expect("We have just build the chain.");
        }

        // update canonical index
        self.block_indices.canonicalize_blocks(new_canon_chain.blocks());

        let chain_action;

        // if joins to the tip;
        if new_canon_chain.fork_block_hash() == old_tip.hash {
            chain_action =
                CanonStateNotification::Commit { new: Arc::new(new_canon_chain.clone()) };
            // append to database
            self.commit_canonical(new_canon_chain)?;
        } else {
            // it forks to canonical block that is not the tip.

            let canon_fork = new_canon_chain.fork_block();
            // sanity check
            if self.block_indices.canonical_hash(&canon_fork.number) != Some(canon_fork.hash) {
                unreachable!("all chains should point to canonical chain.");
            }

            let old_canon_chain = self.revert_canonical(canon_fork.number)?;

            // state action
            chain_action = CanonStateNotification::Reorg {
                old: Arc::new(old_canon_chain.clone()),
                new: Arc::new(new_canon_chain.clone()),
            };

            // commit new canonical chain.
            self.commit_canonical(new_canon_chain)?;

            // insert old canon chain
            self.insert_chain(AppendableChain::new(old_canon_chain));
        }

        // send notification
        let _ = self.canon_state_notification_sender.send(chain_action);

        Ok(())
    }

    /// Subscribe to new blocks events.
    ///
    /// Note: Only canonical blocks are send.
    pub fn subscribe_canon_state(&self) -> CanonStateNotifications {
        self.canon_state_notification_sender.subscribe()
    }

    /// Canonicalize the given chain and commit it to the database.
    fn commit_canonical(&mut self, chain: Chain) -> Result<(), Error> {
        let mut tx = Transaction::new(&self.externals.db)?;

        let (blocks, state) = chain.into_inner();

        tx.append_blocks_with_post_state(blocks.into_blocks().collect(), state)
            .map_err(|e| ExecError::CanonicalCommit { inner: e.to_string() })?;

        tx.commit()?;

        Ok(())
    }

    /// Unwind tables and put it inside state
    pub fn unwind(&mut self, unwind_to: BlockNumber) -> Result<(), Error> {
        // nothing to be done if unwind_to is higher then the tip
        if self.block_indices.canonical_tip().number <= unwind_to {
            return Ok(())
        }
        // revert `N` blocks from current canonical chain and put them inside BlockchanTree
        let old_canon_chain = self.revert_canonical(unwind_to)?;

        // check if there is block in chain
        if old_canon_chain.blocks().is_empty() {
            return Ok(())
        }
        self.block_indices.unwind_canonical_chain(unwind_to);
        // insert old canonical chain to BlockchainTree.
        self.insert_chain(AppendableChain::new(old_canon_chain));

        Ok(())
    }

    /// Revert canonical blocks from the database and return them.
    ///
    /// The block, `revert_until`, is non-inclusive, i.e. `revert_until` stays in the database.
    fn revert_canonical(&mut self, revert_until: BlockNumber) -> Result<Chain, Error> {
        // read data that is needed for new sidechain

        let mut tx = Transaction::new(&self.externals.db)?;

        // read block and execution result from database. and remove traces of block from tables.
        let blocks_and_execution = tx
            .take_block_and_execution_range(
                self.externals.chain_spec.as_ref(),
                (revert_until + 1)..,
            )
            .map_err(|e| ExecError::CanonicalRevert { inner: e.to_string() })?;

        tx.commit()?;

        Ok(Chain::new(blocks_and_execution))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestExecutorFactory;
    use assert_matches::assert_matches;
    use reth_db::{
        mdbx::{test_utils::create_test_rw_db, Env, WriteMap},
        transaction::DbTxMut,
    };
    use reth_interfaces::test_utils::TestConsensus;
    use reth_primitives::{proofs::EMPTY_ROOT, ChainSpecBuilder, H256, MAINNET};
    use reth_provider::{
        insert_block, post_state::PostState, test_utils::blocks::BlockChainTestData,
    };
    use std::{collections::HashSet, sync::Arc};

    fn setup_externals(
        exec_res: Vec<PostState>,
    ) -> TreeExternals<Arc<Env<WriteMap>>, Arc<TestConsensus>, TestExecutorFactory> {
        let db = create_test_rw_db();
        let consensus = Arc::new(TestConsensus::default());
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .shanghai_activated()
                .build(),
        );
        let executor_factory = TestExecutorFactory::new(chain_spec.clone());
        executor_factory.extend(exec_res);

        TreeExternals::new(db, consensus, executor_factory, chain_spec)
    }

    fn setup_genesis<DB: Database>(db: DB, mut genesis: SealedBlock) {
        // insert genesis to db.

        genesis.header.header.number = 10;
        genesis.header.header.state_root = EMPTY_ROOT;
        let tx_mut = db.tx_mut().unwrap();

        insert_block(&tx_mut, genesis, None, false, Some((0, 0))).unwrap();

        // insert first 10 blocks
        for i in 0..10 {
            tx_mut.put::<tables::CanonicalHeaders>(i, H256([100 + i as u8; 32])).unwrap();
        }
        tx_mut.commit().unwrap();
    }

    /// Test data structure that will check tree internals
    #[derive(Default, Debug)]
    struct TreeTester {
        /// Number of chains
        chain_num: Option<usize>,
        /// Check block to chain index
        block_to_chain: Option<HashMap<BlockHash, BlockChainId>>,
        /// Check fork to child index
        fork_to_child: Option<HashMap<BlockHash, HashSet<BlockHash>>>,
        /// Pending blocks
        pending_blocks: Option<(BlockNumber, HashSet<BlockHash>)>,
    }

    impl TreeTester {
        fn with_chain_num(mut self, chain_num: usize) -> Self {
            self.chain_num = Some(chain_num);
            self
        }
        fn with_block_to_chain(mut self, block_to_chain: HashMap<BlockHash, BlockChainId>) -> Self {
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

        fn with_pending_blocks(
            mut self,
            pending_blocks: (BlockNumber, HashSet<BlockHash>),
        ) -> Self {
            self.pending_blocks = Some(pending_blocks);
            self
        }

        fn assert<DB: Database, C: Consensus, EF: ExecutorFactory>(
            self,
            tree: &BlockchainTree<DB, C, EF>,
        ) {
            if let Some(chain_num) = self.chain_num {
                assert_eq!(tree.chains.len(), chain_num);
            }
            if let Some(block_to_chain) = self.block_to_chain {
                assert_eq!(*tree.block_indices.blocks_to_chain(), block_to_chain);
            }
            if let Some(fork_to_child) = self.fork_to_child {
                assert_eq!(*tree.block_indices.fork_to_child(), fork_to_child);
            }
            if let Some(pending_blocks) = self.pending_blocks {
                let (num, hashes) = tree.block_indices.pending_blocks();
                let hashes = hashes.into_iter().collect::<HashSet<_>>();
                assert_eq!((num, hashes), pending_blocks);
            }
        }
    }

    #[tokio::test]
    async fn sanity_path() {
        let data = BlockChainTestData::default();
        let (mut block1, exec1) = data.blocks[0].clone();
        block1.number = 11;
        let (mut block2, exec2) = data.blocks[1].clone();
        block2.number = 12;

        // test pops execution results from vector, so order is from last to first.
        let externals = setup_externals(vec![exec2.clone(), exec1.clone(), exec2, exec1]);

        // last finalized block would be number 9.
        setup_genesis(externals.db.clone(), data.genesis);

        // make tree
        let config = BlockchainTreeConfig::new(1, 2, 3);
        let (sender, mut canon_notif) = tokio::sync::broadcast::channel(10);
        let mut tree =
            BlockchainTree::new(externals, sender, config).expect("failed to create tree");

        // genesis block 10 is already canonical
        assert_eq!(tree.make_canonical(&H256::zero()), Ok(()));

        // insert block2 hits max chain size
        assert_eq!(
            tree.insert_block_with_senders(block2.clone()),
            Err(ExecError::PendingBlockIsInFuture {
                block_number: block2.number,
                block_hash: block2.hash(),
                last_finalized: 9,
            }
            .into())
        );

        // make genesis block 10 as finalized
        tree.finalize_block(10);

        // block 2 parent is not known.
        assert_eq!(tree.insert_block_with_senders(block2.clone()), Ok(BlockStatus::Disconnected));

        // insert block1
        assert_eq!(tree.insert_block_with_senders(block1.clone()), Ok(BlockStatus::Valid));
        // already inserted block will return true.
        assert_eq!(tree.insert_block_with_senders(block1.clone()), Ok(BlockStatus::Valid));

        // insert block2
        assert_eq!(tree.insert_block_with_senders(block2.clone()), Ok(BlockStatus::Valid));

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
            .with_block_to_chain(HashMap::from([(block1.hash, 0), (block2.hash, 0)]))
            .with_fork_to_child(HashMap::from([(block1.parent_hash, HashSet::from([block1.hash]))]))
            .with_pending_blocks((block1.number, HashSet::from([block1.hash])))
            .assert(&tree);

        // make block1 canonical
        assert_eq!(tree.make_canonical(&block1.hash()), Ok(()));
        // check notification
        assert_matches!(canon_notif.try_recv(), Ok(CanonStateNotification::Commit{ new}) if *new.blocks() == BTreeMap::from([(block1.number,block1.clone())]));

        // make block2 canonicals
        assert_eq!(tree.make_canonical(&block2.hash()), Ok(()));
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

        let mut block1a = block1.clone();
        let block1a_hash = H256([0x33; 32]);
        block1a.hash = block1a_hash;
        let mut block2a = block2.clone();
        let block2a_hash = H256([0x34; 32]);
        block2a.hash = block2a_hash;

        // reinsert two blocks that point to canonical chain
        assert_eq!(tree.insert_block_with_senders(block1a.clone()), Ok(BlockStatus::Accepted));

        TreeTester::default()
            .with_chain_num(1)
            .with_block_to_chain(HashMap::from([(block1a_hash, 1)]))
            .with_fork_to_child(HashMap::from([(
                block1.parent_hash,
                HashSet::from([block1a_hash]),
            )]))
            .with_pending_blocks((block2.number + 1, HashSet::from([])))
            .assert(&tree);

        assert_eq!(tree.insert_block_with_senders(block2a.clone()), Ok(BlockStatus::Accepted));
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
            .with_block_to_chain(HashMap::from([(block1a_hash, 1), (block2a_hash, 2)]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1a_hash])),
                (block1.hash(), HashSet::from([block2a_hash])),
            ]))
            .with_pending_blocks((block2.number + 1, HashSet::from([])))
            .assert(&tree);

        // make b2a canonical
        assert_eq!(tree.make_canonical(&block2a_hash), Ok(()));
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
            .with_block_to_chain(HashMap::from([(block1a_hash, 1), (block2.hash, 3)]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1a_hash])),
                (block1.hash(), HashSet::from([block2.hash])),
            ]))
            .with_pending_blocks((block2.number + 1, HashSet::new()))
            .assert(&tree);

        assert_eq!(tree.make_canonical(&block1a_hash), Ok(()));
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
                (block1.hash, 4),
                (block2a_hash, 4),
                (block2.hash, 3),
            ]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1.hash])),
                (block1.hash(), HashSet::from([block2.hash])),
            ]))
            .with_pending_blocks((block1a.number + 1, HashSet::new()))
            .assert(&tree);

        // check notification.
        assert_matches!(canon_notif.try_recv(),
            Ok(CanonStateNotification::Reorg{ old, new})
            if *old.blocks() == BTreeMap::from([(block1.number,block1.clone()),(block2a.number,block2a.clone())])
                && *new.blocks() == BTreeMap::from([(block1a.number,block1a.clone())]));

        // make b2 canonical
        assert_eq!(tree.make_canonical(&block2.hash()), Ok(()));

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
            .with_block_to_chain(HashMap::from([(block1a_hash, 5), (block2a_hash, 4)]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1a_hash])),
                (block1.hash(), HashSet::from([block2a_hash])),
            ]))
            .with_pending_blocks((block2.number + 1, HashSet::new()))
            .assert(&tree);

        // check notification.
        assert_matches!(canon_notif.try_recv(),
            Ok(CanonStateNotification::Reorg{ old, new})
            if *old.blocks() == BTreeMap::from([(block1a.number,block1a.clone())])
                && *new.blocks() == BTreeMap::from([(block1.number,block1.clone()),(block2.number,block2.clone())]));

        // finalize b1 that would make b1a removed from tree
        tree.finalize_block(11);
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
            .with_block_to_chain(HashMap::from([(block2a_hash, 4)]))
            .with_fork_to_child(HashMap::from([(block1.hash(), HashSet::from([block2a_hash]))]))
            .with_pending_blocks((block2.number + 1, HashSet::from([])))
            .assert(&tree);

        // unwind canonical
        assert_eq!(tree.unwind(block1.number), Ok(()));
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
            .with_block_to_chain(HashMap::from([(block2a_hash, 4), (block2.hash, 6)]))
            .with_fork_to_child(HashMap::from([(
                block1.hash(),
                HashSet::from([block2a_hash, block2.hash]),
            )]))
            .with_pending_blocks((block2.number, HashSet::from([block2.hash, block2a.hash])))
            .assert(&tree);

        // commit b2a
        assert_eq!(tree.make_canonical(&block2.hash), Ok(()));

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
            .with_block_to_chain(HashMap::from([(block2a_hash, 4)]))
            .with_fork_to_child(HashMap::from([(block1.hash(), HashSet::from([block2a_hash]))]))
            .with_pending_blocks((block2.number + 1, HashSet::new()))
            .assert(&tree);

        // check notification.
        assert_matches!(canon_notif.try_recv(),
            Ok(CanonStateNotification::Commit{ new})
            if *new.blocks() == BTreeMap::from([(block2.number,block2.clone())]));

        // update canonical block to b2, this would make b2a be removed
        assert_eq!(tree.restore_canonical_hashes(12), Ok(()));
        // Trie state:
        // b2 (finalized)
        // |
        // b1 (finalized)
        // |
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(0)
            .with_block_to_chain(HashMap::from([]))
            .with_fork_to_child(HashMap::from([]))
            .with_pending_blocks((block2.number + 1, HashSet::from([])))
            .assert(&tree);
    }
}
