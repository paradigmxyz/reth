//! Implementation of [`BlockchainTree`]
pub mod block_indices;
pub mod chain;

pub use chain::{Chain, ChainId, ForkBlock};

use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::{consensus::Consensus, executor::Error as ExecError, Error};
use reth_primitives::{BlockHash, BlockNumber, ChainSpec, SealedBlock, SealedBlockWithSenders};
use reth_provider::{
    HeaderProvider, ShareableDatabase, StateProvider, StateProviderFactory, Transaction,
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use self::block_indices::BlockIndices;

#[cfg_attr(doc, aquamarine::aquamarine)]
/// Tree of chains and it identifications.
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
/// block5canon --> block6pending:::pending
/// block5canon --> block67pending:::pending
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
/// * insert_block: insert block inside tree. Execute it and save it to database.
/// * finalize_block: Flush chain that joins to finalized block.
/// * make_canonical: Check if we have the hash of block that we want to finalize and commit it to
///   db. If we dont have the block pipeline syncing should start to fetch the blocks from p2p.
/// *
/// Do reorg if needed
pub struct BlockchainTree<DB, CONSENSUS> {
    /// chains and present data
    pub chains: HashMap<ChainId, Chain>,
    /// Static chain id generator
    pub chain_id_generator: u64,
    /// Indices to block and their connection.
    pub block_indices: BlockIndices,
    /// Number of block after finalized block that we are storing. It should be more then
    /// finalization window
    pub num_of_side_chain_max_size: u64,
    /// Finalization windows. Number of blocks that can be reorged
    pub finalization_window: u64,
    /// Chain spec
    pub chain_spec: Arc<ChainSpec>,
    /// Needs db to save sidechain, do reorgs and push new block to canonical chain that is inside
    /// db.
    pub db: DB,
    /// Consensus
    pub consensus: CONSENSUS,
}

/// Helper structure that wraps chains and indices to search for block hash accross the chains.
pub struct BlockHashes<'a> {
    /// Chains
    pub chains: &'a mut HashMap<ChainId, Chain>,
    /// Indices
    pub indices: &'a BlockIndices,
}

impl<DB: Database, CONSENSUS: Consensus> BlockchainTree<DB, CONSENSUS> {
    /// New blockchain tree
    pub fn new(
        db: DB,
        consensus: CONSENSUS,
        chain_spec: Arc<ChainSpec>,
        finalization_window: u64,
        num_of_side_chain_max_size: u64,
        num_of_additional_canonical_block_hashes: u64,
    ) -> Result<Self, Error> {
        if num_of_side_chain_max_size > finalization_window {
            panic!("Side chain size should be more then finalization window");
        }

        let last_canonical_hashes = db
            .tx()?
            .cursor_read::<tables::CanonicalHeaders>()?
            .walk_back(None)?
            .take((finalization_window + num_of_additional_canonical_block_hashes) as usize)
            .collect::<Result<Vec<(BlockNumber, BlockHash)>, _>>()?;

        // TODO(rakita) save last finalized block inside database but for now just take
        // tip-finalization_window
        let (last_finalized_block_number, _) =
            if last_canonical_hashes.len() > finalization_window as usize {
                last_canonical_hashes[finalization_window as usize]
            } else {
                // it is in reverse order from tip to N
                last_canonical_hashes.last().cloned().unwrap_or_default()
            };

        Ok(Self {
            db,
            consensus,
            chain_spec,
            chain_id_generator: 0,
            chains: Default::default(),
            block_indices: BlockIndices::new(
                last_finalized_block_number,
                num_of_additional_canonical_block_hashes,
            ),
            num_of_side_chain_max_size,
            finalization_window,
        })
    }

    /// Fork side chain or append the block if parent is the top of the chain
    fn fork_side_chain(
        &mut self,
        block: SealedBlockWithSenders,
        chain_id: ChainId,
    ) -> Result<(), Error> {
        let block_hashes = self.all_chain_hashes(chain_id);

        // get canonical fork.
        let canonical_fork =
            self.canonical_fork(chain_id).ok_or(ExecError::ChainIdConsistency { chain_id })?;

        // get chain that block needs to join to.
        let parent_chain =
            self.chains.get_mut(&chain_id).ok_or(ExecError::ChainIdConsistency { chain_id })?;
        let chain_tip = parent_chain.tip().hash();

        let canonical_block_hashes = self.block_indices.canonical_chain();

        // get canonical tip
        let (_, canonical_tip_hash) =
            canonical_block_hashes.last_key_value().map(|(i, j)| (*i, *j)).unwrap_or_default();

        let db = ShareableDatabase::new(&self.db, self.chain_spec.clone());
        let provider = if canonical_fork.hash == canonical_tip_hash {
            Box::new(db.latest()?) as Box<dyn StateProvider>
        } else {
            Box::new(db.history_by_block_number(canonical_fork.number)?) as Box<dyn StateProvider>
        };

        // append the block if it is continuing the chain.
        if chain_tip == block.parent_hash {
            parent_chain.append_block(
                block,
                block_hashes,
                canonical_block_hashes,
                self.chain_spec.clone(),
                &provider,
                &self.consensus,
            )?;
        } else {
            let chain = parent_chain.new_chain_fork(
                block,
                block_hashes,
                canonical_block_hashes,
                self.chain_spec.clone(),
                &provider,
                &self.consensus,
            )?;
            // release the lifetime with a drop
            drop(provider);
            self.insert_chain(chain);
        }

        Ok(())
    }

    /// Fork canonical chain by creating new chain
    pub fn fork_canonical_chain(&mut self, block: SealedBlockWithSenders) -> Result<(), Error> {
        let canonical_block_hashes = self.block_indices.canonical_chain();
        let (_, canonical_tip) =
            canonical_block_hashes.last_key_value().map(|(i, j)| (*i, *j)).unwrap_or_default();

        // create state provider
        let db = ShareableDatabase::new(&self.db, self.chain_spec.clone());
        let parent_header = db
            .header(&block.parent_hash)?
            .ok_or(ExecError::CanonicalChain { block_hash: block.parent_hash })?;

        let provider = if block.parent_hash == canonical_tip {
            Box::new(db.latest()?) as Box<dyn StateProvider>
        } else {
            Box::new(db.history_by_block_number(block.number - 1)?) as Box<dyn StateProvider>
        };

        let parent_header = parent_header.seal(block.parent_hash);
        let chain = Chain::new_canonical_fork(
            &block,
            parent_header,
            canonical_block_hashes,
            self.chain_spec.clone(),
            &provider,
            &self.consensus,
        )?;
        drop(provider);
        self.insert_chain(chain);
        Ok(())
    }

    /// Get all block hashes from chain that are not canonical. This is one time operation per
    /// block. Reason why this is not caches is to save memory.
    fn all_chain_hashes(&self, chain_id: ChainId) -> BTreeMap<BlockNumber, BlockHash> {
        // find chain and iterate over it,
        let mut chain_id = chain_id;
        let mut hashes = BTreeMap::new();
        loop {
            let Some(chain) = self.chains.get(&chain_id) else { return hashes};
            hashes.extend(chain.blocks.values().map(|b| (b.number, b.hash())));

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

    /// Getting the canonical fork would tell use what kind of Provider we should execute block on.
    /// If it is latest state provider or history state provider
    /// Return None if chain_id is not known.
    fn canonical_fork(&self, chain_id: ChainId) -> Option<ForkBlock> {
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

    /// Insert chain to tree and ties the blocks to it.
    /// Helper function that handles indexing and inserting.
    fn insert_chain(&mut self, chain: Chain) -> ChainId {
        let chain_id = self.chain_id_generator;
        self.chain_id_generator += 1;
        self.block_indices.insert_chain(chain_id, &chain);
        // add chain_id -> chain index
        self.chains.insert(chain_id, chain);
        chain_id
    }

    /// Insert block inside tree. recover transaction signers and call [`insert_block_with_senders`]
    /// fn.
    pub fn insert_block(&mut self, block: SealedBlock) -> Result<bool, Error> {
        let senders = block.senders().ok_or(ExecError::SenderRecoveryError)?;
        let block = SealedBlockWithSenders::new(block, senders).unwrap();
        self.insert_block_with_senders(&block)
    }

    /// Insert block with senders inside tree
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
        if block.number > last_finalized_block + self.num_of_side_chain_max_size {
            return Err(ExecError::PendingBlockIsInFuture {
                block_number: block.number,
                block_hash: block.hash(),
                last_finalized: last_finalized_block,
            }
            .into())
        }

        // check if block parent can be found in Tree
        if let Some(parent_chain) = self.block_indices.get_blocks_chain_id(&block.parent_hash) {
            self.fork_side_chain(block.clone(), parent_chain)?;
            //self.db.tx_mut()?.put::<tables::PendingBlocks>(block.hash(), block.unseal())?;
            return Ok(true)
        }

        // if not found, check if it can be found inside canonical chain.
        if Some(block.parent_hash) == self.block_indices.canonical_hash(&(block.number - 1)) {
            // create new chain that points to that block
            self.fork_canonical_chain(block.clone())?;
            //self.db.tx_mut()?.put::<tables::PendingBlocks>(block.hash(), block.unseal())?;
            return Ok(true)
        }
        // NOTE: Block doesn't have a parent, and if we receive this block in `make_canonical`
        // function this could be a trigger to initiate p2p syncing, as we are missing the
        // parent.
        Ok(false)
    }

    /// Do finalization of blocks. Remove them from tree
    pub fn finalize_block(&mut self, finalized_block: BlockNumber) {
        let mut remove_chains = self.block_indices.finalize_canonical_blocks(finalized_block);

        while let Some(chain_id) = remove_chains.first() {
            if let Some(chain) = self.chains.remove(chain_id) {
                remove_chains.extend(self.block_indices.remove_chain(&chain));
            }
        }
    }

    /// Update canonical hashes. Reads last N canonical blocks from database and update all indices.
    pub fn update_canonical_hashes(
        &mut self,
        last_finalized_block: BlockNumber,
    ) -> Result<(), Error> {
        self.finalize_block(last_finalized_block);

        let num_of_canonical_hashes =
            self.finalization_window + self.block_indices.num_of_additional_canonical_block_hashes;

        let last_canonical_hashes = self
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

    /// Make block and its parent canonical. Unwind chains to database if necessary.
    pub fn make_canonical(&mut self, block_hash: &BlockHash) -> Result<(), Error> {
        let chain_id = self
            .block_indices
            .get_blocks_chain_id(block_hash)
            .ok_or(ExecError::BlockHashNotFoundInChain { block_hash: *block_hash })?;
        let chain = self.chains.remove(&chain_id).expect("To be present");

        // we are spliting chain as there is possibility that only part of chain get canonicalized.
        let (canonical, pending) = chain.split_at_block_hash(block_hash);
        let canonical = canonical.expect("Canonical chain is present");

        //
        if let Some(pending) = pending {
            // fork is now canonical and latest.
            self.chains.insert(chain_id, pending);
        }

        let mut block_fork = canonical.fork_block();
        let mut block_fork_number = canonical.fork_block_number();
        let mut chains_to_promote = vec![canonical];

        // loop while fork blocks are found in Tree.
        while let Some(chain_id) = self.block_indices.get_blocks_chain_id(&block_fork.hash) {
            let chain = self.chains.remove(&chain_id).expect("To fork to be present");
            block_fork = chain.fork_block();
            let (canonical, rest) = chain.split_at_number(block_fork_number);
            let canonical = canonical.expect("Chain is present");
            // reinsert back the chunk of sidechain that didn't get reorged.
            if let Some(rest_of_sidechain) = rest {
                self.chains.insert(chain_id, rest_of_sidechain);
            }
            block_fork_number = canonical.fork_block_number();
            chains_to_promote.push(canonical);
        }

        let old_tip = self.block_indices.canonical_tip();
        // Merge all chain into one chain.
        let mut new_canon_chain = chains_to_promote.pop().expect("There is at least one block");
        for chain in chains_to_promote.into_iter().rev() {
            new_canon_chain.append_chain(chain);
        }

        // update canonical index
        self.block_indices.canonicalize_blocks(&new_canon_chain.blocks);

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

            // insert old canonical chain to BlockchainTree.
            self.insert_chain(old_canon_chain);
        }

        Ok(())
    }

    /// Commit chain for it to become canonical. Assume we are doing pending operation to db.
    fn commit_canonical(&mut self, chain: Chain) -> Result<(), Error> {
        let mut tx = Transaction::new(&self.db)?;

        let new_tip = chain.tip().number;

        for item in chain.blocks.into_iter().zip(chain.changesets.into_iter()) {
            let ((_, block), changeset) = item;

            tx.insert_block(&block, &self.chain_spec, changeset)
                .map_err(|_| ExecError::VerificationFailed)?;
        }
        // update pipeline progress.
        tx.update_pipeline_stages(new_tip).map_err(|_| ExecError::VerificationFailed)?;

        // TODO error cast

        tx.commit()?;

        Ok(())
    }

    /// Revert canonical blocks from database and insert them to pending table
    /// Revert should be non inclusive, and revert_until should stay in db.
    /// Return the chain that represent reverted canonical blocks.
    fn revert_canonical(&mut self, revert_until: BlockNumber) -> Result<Chain, Error> {
        // read data that is needed for new sidechain

        let mut tx = Transaction::new(&self.db)?;

        // read block and execution result from database. and remove traces of block from tables.
        let blocks_and_execution = tx
            .get_block_and_execution_range::<true>(revert_until + 1..)
            .map_err(|_| ExecError::VerificationFailed)?;

        // update pipeline progress.
        tx.update_pipeline_stages(revert_until).map_err(|_| ExecError::VerificationFailed)?;
        // TODO error cast

        tx.commit()?;

        let chain = Chain::new(blocks_and_execution);

        Ok(chain)
    }
}
