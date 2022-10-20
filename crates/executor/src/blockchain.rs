use crate::revm_wrap::{State, SubState};
use crate::Config;
use eyre::eyre;
use reth_primitives::{Block, BlockID, BlockLocked, BlockNumber, Header};
use std::cmp::max;
use std::collections::HashMap;

/// Blockchain interface
pub struct Blockchain<'a> {
    /// Genesis block.
    genesis: Header,
    /// Best block number.
    chain_heigh: BlockNumber,
    /// Blocks maped by block hash.
    blocks: HashMap<BlockID, BlockLocked>,
    /// Canonical chain mapping BlockNumber with Block hash.
    chain: HashMap<BlockNumber, BlockID>,
    /// State
    state: SubState<'a>,
    /// Config
    config: Config,
}

impl<'a> Blockchain<'a> {
    /// Create new blockchain.
    pub fn new(genesis: Header, config: Config, state: State<'a>) -> Self {
        Self {
            genesis,
            chain_heigh: 0,
            blocks: HashMap::new(),
            chain: HashMap::new(),
            state: SubState::new(state),
            config,
        }
    }

    /// Expose internal state
    pub fn substate(&mut self) -> &mut SubState<'a> {
        &mut self.state
    }

    /// Push block on the chain without checks
    pub fn push_unchecked_block(&mut self, block: BlockLocked) {
        let number = block.number;
        let id = block.header.hash();

        self.blocks.insert(id, block);
        self.chain.insert(number, id);
        self.chain_heigh = max(self.chain_heigh, number);
    }

    /// Before execution do verification on header and body if it can be included
    /// inside blockchain. TODO see where this should be placed.
    pub async fn pre_verification(&self, block: BlockLocked) -> eyre::Result<()> {
        let id = block.header.hash();

        // Chain height needs to be more then current number.
        if self.chain_heigh + 1 < block.number {
            return Err(eyre!("Block number is in far future"));
        }

        // Check if block is known.
        if self.blocks.contains_key(&id) {
            return Err(eyre!("Block known"));
        }

        // Check if parent is known.
        if let Some(parent) = self.blocks.get(&block.parent_hash) {
            // And if block number is consistent.
            if parent.number + 1 != block.number {
                return Err(eyre!("There is gap with parent block number"));
            }
        } else {
            return Err(eyre!("Parent block unknown"));
        }

        // Gas used needs to be less then gas limit. Gas used is going to be check after execution.
        if block.gas_used > block.gas_limit {
            return Err(eyre!("Block gas used is greater then gas limit"));
        }

        // check omners hash
        let omners_hash =
            crate::proofs::calculate_omners_root(block.omners.iter().map(|h| h.as_ref()));
        if block.header.ommers_hash != omners_hash {
            return Err(eyre!("Omner hash is different"));
        }
        let transaction_root = crate::proofs::calculate_transaction_root(block.body.iter());

        // TODO transaction root
        // TODO receipts_root if present

        // TODO  consensus
        //  * difficulty
        //  * gas_limit with basefee
        //  * timestamp
        //  * mix_hash & nonce PoW stuf
        //  * extra_data

        Ok(())
    }

    /// Verification after block is executed
    pub async fn post_verification(&mut self, block: BlockLocked) -> eyre::Result<()> {
        // TODO block.state_root;
        // block.logs_bloom;

        // block.gas_used & if limit is hit
        Ok(())
    }

    /// Execute and push block, return error if execution or verification didn't pass.
    pub fn push_block(&mut self, block: BlockLocked) -> eyre::Result<()> {
        Ok(())
    }
}
