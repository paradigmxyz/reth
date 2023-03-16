//! Implementation of [`BlockchainTree`]
pub mod block_indices;
pub mod chain;

use self::{
    block_indices::BlockIndices,
    chain::{ChainSplit, SplitAt},
};
use chain::{BlockChainId, Chain, ForkBlock};
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::{consensus::Consensus, executor::Error as ExecError, Error};
use reth_primitives::{BlockHash, BlockNumber, ChainSpec, SealedBlock, SealedBlockWithSenders};
use reth_provider::{
    providers::ChainState, ExecutorFactory, HeaderProvider, ShareableDatabase,
    StateProviderFactory, Transaction,
};
use std::{
    collections::{BTreeMap, HashMap},
    ops::DerefMut,
    sync::Arc,
};

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
/// * insert_block: Connect block to chain, execute it and if valid insert block inside tree.
/// * finalize_block: Remove chains that join to now finalized block, as chain becomes invalid.
/// * make_canonical: Check if we have the hash of block that we want to finalize and commit it to
///   db. If we dont have the block, pipeline syncing should start to fetch the blocks from p2p. Do
///   reorg in tables if canonical chain if needed.

pub struct BlockchainTree<DB: Database, C: Consensus, EF: ExecutorFactory> {
    /// The tracked chains and their current data.
    chains: HashMap<BlockChainId, Chain>,
    /// Static blockchain ID generator
    block_chain_id_generator: u64,
    /// Indices to block and their connection to the canonical chain.
    block_indices: BlockIndices,
    /// Number of blocks after the last finalized block that we are storing.
    ///
    /// It should be more than the finalization window for the canonical chain.
    max_blocks_in_chain: u64,
    /// The number of blocks that can be re-orged (finalization windows)
    max_reorg_depth: u64,
    /// External components (the database, consensus engine etc.)
    externals: Externals<DB, C, EF>,
}

/// A container for external components.
///
/// This is a simple container for external components used throughout the blockchain tree
/// implementation:
///
/// - A handle to the database
/// - A handle to the consensus engine
/// - The executor factory to exexcute blocks with
/// - The chain spec
struct Externals<DB: Database, C: Consensus, EF: ExecutorFactory> {
    /// The database, used to commit the canonical chain, or unwind it.
    db: DB,
    /// The consensus engine.
    consensus: C,
    /// The executor factory to execute blocks with.
    executor_factory: EF,
    /// The chain spec.
    chain_spec: Arc<ChainSpec>,
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> Externals<DB, C, EF> {
    /// Get a [`ShareableDatabase`] instance pointing to the underlying database.
    fn sharable_db(&self) -> ShareableDatabase<&DB> {
        ShareableDatabase::new(&self.db, self.chain_spec.clone())
    }
}

/// A container that wraps chains and block indices to allow searching for block hashes across all
/// sidechains.
pub struct BlockHashes<'a> {
    /// The current tracked chains.
    pub chains: &'a mut HashMap<BlockChainId, Chain>,
    /// The block indices for all chains.
    pub indices: &'a BlockIndices,
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTree<DB, C, EF> {
    /// Create a new blockchain tree.
    pub fn new(
        db: DB,
        consensus: C,
        executor_factory: EF,
        chain_spec: Arc<ChainSpec>,
        max_reorg_depth: u64,
        max_blocks_in_chain: u64,
        num_of_additional_canonical_block_hashes: u64,
    ) -> Result<Self, Error> {
        if max_reorg_depth > max_blocks_in_chain {
            panic!("Side chain size should be more then finalization window");
        }

        let last_canonical_hashes = db
            .tx()?
            .cursor_read::<tables::CanonicalHeaders>()?
            .walk_back(None)?
            .take((max_reorg_depth + num_of_additional_canonical_block_hashes) as usize)
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

        let externals = Externals { db, consensus, executor_factory, chain_spec };

        Ok(Self {
            externals,
            block_chain_id_generator: 0,
            chains: Default::default(),
            block_indices: BlockIndices::new(
                last_finalized_block_number,
                num_of_additional_canonical_block_hashes,
                BTreeMap::from_iter(last_canonical_hashes.into_iter()),
            ),
            max_blocks_in_chain,
            max_reorg_depth,
        })
    }

    /// Create a new sidechain by forking the given chain, or append the block if the parent block
    /// is the top of the given chain.
    fn fork_side_chain(
        &mut self,
        block: SealedBlockWithSenders,
        chain_id: BlockChainId,
    ) -> Result<(), Error> {
        let block_hashes = self.all_chain_hashes(chain_id);

        // get canonical fork.
        let canonical_fork =
            self.canonical_fork(chain_id).ok_or(ExecError::BlockChainIdConsistency { chain_id })?;

        // get chain that block needs to join to.
        let parent_chain = self
            .chains
            .get_mut(&chain_id)
            .ok_or(ExecError::BlockChainIdConsistency { chain_id })?;
        let chain_tip = parent_chain.tip().hash();

        let canonical_block_hashes = self.block_indices.canonical_chain();

        // get canonical tip
        let (_, canonical_tip_hash) =
            canonical_block_hashes.last_key_value().map(|(i, j)| (*i, *j)).unwrap_or_default();

        let db = self.externals.sharable_db();
        let provider = if canonical_fork.hash == canonical_tip_hash {
            ChainState::boxed(db.latest()?)
        } else {
            ChainState::boxed(db.history_by_block_number(canonical_fork.number)?)
        };

        // append the block if it is continuing the chain.
        if chain_tip == block.parent_hash {
            let block_hash = block.hash();
            let block_number = block.number;
            parent_chain.append_block(
                block,
                block_hashes,
                canonical_block_hashes,
                &provider,
                &self.externals.consensus,
                &self.externals.executor_factory,
            )?;
            drop(provider);
            self.block_indices.insert_non_fork_block(block_number, block_hash, chain_id)
        } else {
            let chain = parent_chain.new_chain_fork(
                block,
                block_hashes,
                canonical_block_hashes,
                &provider,
                &self.externals.consensus,
                &self.externals.executor_factory,
            )?;
            // release the lifetime with a drop
            drop(provider);
            self.insert_chain(chain);
        }

        Ok(())
    }

    /// Create a new sidechain by forking the canonical chain.
    // TODO(onbjerg): Is this not a specialized case of [`fork_side_chain`]? If so, can we merge?
    pub fn fork_canonical_chain(&mut self, block: SealedBlockWithSenders) -> Result<(), Error> {
        let canonical_block_hashes = self.block_indices.canonical_chain();
        let (_, canonical_tip) =
            canonical_block_hashes.last_key_value().map(|(i, j)| (*i, *j)).unwrap_or_default();

        // create state provider
        let db = self.externals.sharable_db();
        let parent_header = db
            .header(&block.parent_hash)?
            .ok_or(ExecError::CanonicalChain { block_hash: block.parent_hash })?;

        let provider = if block.parent_hash == canonical_tip {
            ChainState::boxed(db.latest()?)
        } else {
            ChainState::boxed(db.history_by_block_number(block.number - 1)?)
        };

        let parent_header = parent_header.seal(block.parent_hash);
        let chain = Chain::new_canonical_fork(
            &block,
            &parent_header,
            canonical_block_hashes,
            &provider,
            &self.externals.consensus,
            &self.externals.executor_factory,
        )?;
        drop(provider);
        self.insert_chain(chain);
        Ok(())
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
        if self.block_indices.canonical_hash(&fork.number) == Some(fork.hash) {
            Some(fork)
        } else {
            None
        }
    }

    /// Insert a chain into the tree.
    ///
    /// Inserts a chain into the tree and builds the block indices.
    fn insert_chain(&mut self, chain: Chain) -> BlockChainId {
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
    pub fn insert_block(&mut self, block: SealedBlock) -> Result<bool, Error> {
        let block = block.seal_with_senders().ok_or(ExecError::SenderRecoveryError)?;
        self.insert_block_with_senders(&block)
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
        block: &SealedBlockWithSenders,
    ) -> Result<bool, Error> {
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
        if block.number > last_finalized_block + self.max_blocks_in_chain {
            return Err(ExecError::PendingBlockIsInFuture {
                block_number: block.number,
                block_hash: block.hash(),
                last_finalized: last_finalized_block,
            }
            .into())
        }

        // check if block is already inside Tree
        if self.block_indices.contains_pending_block_hash(block.hash()) {
            // block is known return that is inserted
            return Ok(true)
        }

        // check if block is part of canonical chain
        if self.block_indices.canonical_hash(&block.number) == Some(block.hash()) {
            // block is part of canonical chain
            return Ok(true)
        }

        // check if block parent can be found in Tree
        if let Some(parent_chain) = self.block_indices.get_blocks_chain_id(&block.parent_hash) {
            self.fork_side_chain(block.clone(), parent_chain)?;
            // TODO save pending block to database
            // https://github.com/paradigmxyz/reth/issues/1713
            return Ok(true)
        }

        // if not found, check if the parent can be found inside canonical chain.
        if Some(block.parent_hash) == self.block_indices.canonical_hash(&(block.number - 1)) {
            // create new chain that points to that block
            self.fork_canonical_chain(block.clone())?;
            // TODO save pending block to database
            // https://github.com/paradigmxyz/reth/issues/1713
            return Ok(true)
        }
        // NOTE: Block doesn't have a parent, and if we receive this block in `make_canonical`
        // function this could be a trigger to initiate p2p syncing, as we are missing the
        // parent.
        Ok(false)
    }

    /// Finalize blocks up until and including `finalized_block`, and remove them from the tree.
    pub fn finalize_block(&mut self, finalized_block: BlockNumber) {
        let mut remove_chains = self.block_indices.finalize_canonical_blocks(finalized_block);

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
    pub fn update_canonical_hashes(
        &mut self,
        last_finalized_block: BlockNumber,
    ) -> Result<(), Error> {
        self.finalize_block(last_finalized_block);

        let num_of_canonical_hashes =
            self.max_reorg_depth + self.block_indices.num_of_additional_canonical_block_hashes();

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
    fn split_chain(&mut self, chain_id: BlockChainId, chain: Chain, split_at: SplitAt) -> Chain {
        match chain.split(split_at) {
            ChainSplit::Split { canonical, pending } => {
                // rest of splited chain is inserted back with same chain_id.
                self.block_indices.insert_chain(chain_id, &pending);
                self.chains.insert(chain_id, pending);
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
        let chain_id = if let Some(chain_id) = self.block_indices.get_blocks_chain_id(block_hash) {
            chain_id
        } else {
            // If block is already canonical don't return error.
            if self.block_indices.is_block_hash_canonical(block_hash) {
                return Ok(())
            }
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

        // if joins to the tip
        if new_canon_chain.fork_block_hash() == old_tip.hash {
            // append to database
            self.commit_canonical(new_canon_chain)?;
        } else {
            // it forks to canonical block that is not the tip.

            let canon_fork = new_canon_chain.fork_block();
            // sanity check
            if self.block_indices.canonical_hash(&canon_fork.number) != Some(canon_fork.hash) {
                unreachable!("all chains should point to canonical chain.");
            }

            // revert `N` blocks from current canonical chain and put them inside BlockchanTree
            // This is main reorgs on tables.
            let old_canon_chain = self.revert_canonical(canon_fork.number)?;
            self.commit_canonical(new_canon_chain)?;

            // TODO we can potentially merge now reverted canonical chain with
            // one of the chain from the tree. Low priority.

            // insert old canonical chain to BlockchainTree.
            self.insert_chain(old_canon_chain);
        }

        Ok(())
    }

    /// Canonicalize the given chain and commit it to the database.
    fn commit_canonical(&mut self, chain: Chain) -> Result<(), Error> {
        let mut tx = Transaction::new(&self.externals.db)?;
        let new_tip = chain.tip().number;
        let first_transition_id =
            tx.get_block_transition(chain.first().number.saturating_sub(1))
                .map_err(|e| ExecError::CanonicalCommit { inner: e.to_string() })?;
        let (blocks, state) = chain.into_inner();

        // Write state and changesets to the database
        state
            .write_to_db(tx.deref_mut(), first_transition_id)
            .map_err(|e| ExecError::CanonicalCommit { inner: e.to_string() })?;

        // Insert the blocks
        for (_, block) in blocks.into_iter() {
            tx.insert_block(block)
                .map_err(|e| ExecError::CanonicalCommit { inner: e.to_string() })?;
        }

        // Update pipeline progress
        tx.update_pipeline_stages(new_tip)
            .map_err(|e| ExecError::PipelineStatusUpdate { inner: e.to_string() })?;

        tx.commit()?;

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

        // update pipeline progress.
        tx.update_pipeline_stages(revert_until)
            .map_err(|e| ExecError::PipelineStatusUpdate { inner: e.to_string() })?;

        tx.commit()?;

        // TODO
        //let chain = Chain::new(blocks_and_execution);
        //
        //Ok(chain)
        Ok(Chain::new(vec![]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use reth_db::{
        mdbx::{test_utils::create_test_rw_db, Env, WriteMap},
        transaction::DbTxMut,
    };
    use reth_interfaces::test_utils::TestConsensus;
    use reth_primitives::{hex_literal::hex, proofs::EMPTY_ROOT, ChainSpecBuilder, H256, MAINNET};
    use reth_provider::{
        insert_block, post_state::PostState, test_utils::blocks::BlockChainTestData, BlockExecutor,
        StateProvider,
    };
    use std::collections::HashSet;

    struct TestFactory {
        exec_result: Arc<Mutex<Vec<PostState>>>,
        chain_spec: Arc<ChainSpec>,
    }

    impl TestFactory {
        fn new(chain_spec: Arc<ChainSpec>) -> Self {
            Self { exec_result: Arc::new(Mutex::new(Vec::new())), chain_spec }
        }

        fn extend(&self, exec_res: Vec<PostState>) {
            self.exec_result.lock().extend(exec_res.into_iter());
        }
    }

    struct TestExecutor(Option<PostState>);

    impl<SP: StateProvider> BlockExecutor<SP> for TestExecutor {
        fn execute(
            &mut self,
            _block: &reth_primitives::Block,
            _total_difficulty: reth_primitives::U256,
            _senders: Option<Vec<reth_primitives::Address>>,
        ) -> Result<PostState, ExecError> {
            self.0.clone().ok_or(ExecError::VerificationFailed)
        }

        fn execute_and_verify_receipt(
            &mut self,
            _block: &reth_primitives::Block,
            _total_difficulty: reth_primitives::U256,
            _senders: Option<Vec<reth_primitives::Address>>,
        ) -> Result<PostState, ExecError> {
            self.0.clone().ok_or(ExecError::VerificationFailed)
        }
    }

    impl ExecutorFactory for TestFactory {
        type Executor<T: StateProvider> = TestExecutor;

        fn with_sp<SP: StateProvider>(&self, _sp: SP) -> Self::Executor<SP> {
            let exec_res = self.exec_result.lock().pop();
            TestExecutor(exec_res)
        }

        fn chain_spec(&self) -> &ChainSpec {
            self.chain_spec.as_ref()
        }
    }

    type TestExternals = (Arc<Env<WriteMap>>, TestConsensus, TestFactory, Arc<ChainSpec>);

    fn externals(exec_res: Vec<PostState>) -> TestExternals {
        let db = create_test_rw_db();
        let consensus = TestConsensus::default();
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .shanghai_activated()
                .build(),
        );
        let executor_factory = TestFactory::new(chain_spec.clone());
        executor_factory.extend(exec_res);

        (db, consensus, executor_factory, chain_spec)
    }

    fn setup(mut genesis: SealedBlock, externals: &TestExternals) {
        // insert genesis to db.

        genesis.header.header.number = 10;
        genesis.header.header.state_root = EMPTY_ROOT;
        let tx_mut = externals.0.tx_mut().unwrap();

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
        }
    }

    #[test]
    fn sanity_path() {
        let data = BlockChainTestData::default();
        let (mut block1, exec1) = data.blocks[0].clone();
        block1.number = 11;
        block1.state_root =
            H256(hex!("5d035ccb3e75a9057452ff060b773b213ec1fc353426174068edfc3971a0b6bd"));
        let (mut block2, exec2) = data.blocks[1].clone();
        block2.number = 12;
        block2.state_root =
            H256(hex!("90101a13dd059fa5cca99ed93d1dc23657f63626c5b8f993a2ccbdf7446b64f8"));

        // test pops execution results from vector, so order is from last to first.ß
        let externals = externals(vec![exec2.clone(), exec1.clone(), exec2, exec1]);

        // last finalized block would be number 9.
        setup(data.genesis, &externals);

        // make tree
        let (db, consensus, exec_factory, chain_spec) = externals;
        let mut tree =
            BlockchainTree::new(db, consensus, exec_factory, chain_spec, 1, 2, 3).unwrap();

        // genesis block 10 is already canonical
        assert_eq!(tree.make_canonical(&H256::zero()), Ok(()));

        // insert block2 hits max chain size
        assert_eq!(
            tree.insert_block_with_senders(&block2),
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
        assert_eq!(tree.insert_block_with_senders(&block2), Ok(false));

        // insert block1
        assert_eq!(tree.insert_block_with_senders(&block1), Ok(true));
        // already inserted block will return true.
        assert_eq!(tree.insert_block_with_senders(&block1), Ok(true));

        // insert block2
        assert_eq!(tree.insert_block_with_senders(&block2), Ok(true));

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
            .assert(&tree);

        // make block1 canonical
        assert_eq!(tree.make_canonical(&block1.hash()), Ok(()));
        // make block2 canonical
        assert_eq!(tree.make_canonical(&block2.hash()), Ok(()));

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
        assert_eq!(tree.insert_block_with_senders(&block1a), Ok(true));

        TreeTester::default()
            .with_chain_num(1)
            .with_block_to_chain(HashMap::from([(block1a_hash, 1)]))
            .with_fork_to_child(HashMap::from([(
                block1.parent_hash,
                HashSet::from([block1a_hash]),
            )]))
            .assert(&tree);

        assert_eq!(tree.insert_block_with_senders(&block2a), Ok(true));
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
            .assert(&tree);

        // make b2a canonical
        assert_eq!(tree.make_canonical(&block2a_hash), Ok(()));
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
            .assert(&tree);

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
            .assert(&tree);

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
            .assert(&tree);

        // update canonical block to b2, this would make b2a be removed
        assert_eq!(tree.update_canonical_hashes(12), Ok(()));
        // Trie state:
        // b2 (canon)
        // |
        // b1 (canon)
        // |
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(0)
            .with_block_to_chain(HashMap::from([]))
            .with_fork_to_child(HashMap::from([]))
            .assert(&tree);
    }
}
