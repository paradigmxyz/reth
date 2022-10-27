//! Blockchain is used as utility structure to chain multiple blocks into one block
use crate::{
    revm_wrap::{State, SubState},
    Config,
};
use eyre::eyre;
use reth_interfaces::executor::ExecutorDb;
use reth_primitives::{Block, BlockID, BlockLocked, BlockNumber, Header, HeaderLocked, H256};
use std::{cmp::max, collections::HashMap};

/// Blockchain interface
pub struct Blockchain {
    /// Genesis block.
    genesis: Header,
    /// Best block number.
    chain_heigh: BlockNumber,
    /// Blocks maped by block hash.
    blocks: HashMap<BlockID, BlockLocked>,
    /// Canonical chain mapping BlockNumber with Block hash.
    chain: HashMap<BlockNumber, BlockID>,
    /// Config
    config: Config,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Temp error")]
    HeaderFaileddd,
    #[error("Block used gas ({gas_used:?}) is greater then gas limit ({gas_limit:?})")]
    HeaderGasUsedExceedsGasLimit { gas_used: u64, gas_limit: u64 },
    #[error("Block ommner hash ({got:?}) is different then expected: ({expected:?})")]
    BodyOmmnersHashDiff { got: H256, expected: H256 },
    #[error("Block transaction root ({got:?}) is different then expected: ({expected:?})")]
    BodyTransactionRootDiff { got: H256, expected: H256 },
    #[error("Block receipts root ({got:?}) is different then expected: ({expected:?})")]
    BodyReceiptsRootDiff { got: H256, expected: H256 },
}

/// All checks that engine needs to do.
/// They are deviden on two big parts:
/// * Pre execution:
///     * Header:
///         * Check all fields and hashed
///         * Check link to parent block: base_fee, block_hash,block_number
///     * Body (Block):
///         * Omners: Check Omners
///         * Transaction: calculate transaction root
///
/// * Post execution
///     * Body (Block):
///         * Receipts: calculate root.
///         * gas used

/// Validate header standalone
pub fn validate_header_standalone(header: &HeaderLocked) -> Result<(), Error> {
    // Gas used needs to be less then gas limit. Gas used is going to be check after execution.
    if header.gas_used > header.gas_limit {
        return Err(Error::HeaderGasUsedExceedsGasLimit {
            gas_used: header.gas_used,
            gas_limit: header.gas_limit,
        })
    }

    Ok(())
}

/// Validate block standalone
pub fn validate_block_standalone(block: &BlockLocked) -> Result<(), Error> {
    // check omners hash
    let omners_hash = crate::proofs::calculate_omners_root(block.omners.iter().map(|h| h.as_ref()));
    if block.header.ommers_hash != omners_hash {
        return Err(Error::BodyOmmnersHashDiff {
            got: omners_hash,
            expected: block.header.ommers_hash,
        })
    }

    // check transaction root
    let transaction_root = crate::proofs::calculate_transaction_root(block.body.iter());
    if block.header.transactions_root != transaction_root {
        return Err(Error::BodyTransactionRootDiff {
            got: transaction_root,
            expected: block.header.transactions_root,
        })
    }

    // check receipts root
    let receipts_root = crate::proofs::calculate_receipt_root(block.receipts.iter());
    if block.header.receipts_root != receipts_root {
        return Err(Error::BodyReceiptsRootDiff {
            got: transaction_root,
            expected: block.header.transactions_root,
        })
    }

    Ok(())
}

/// Validate header in regards to parent
pub fn validate_header_regarding_parent(
    parent: &HeaderLocked,
    block: &HeaderLocked,
) -> Result<(), Error> {
    Ok(())
}

/// Validate block in regards to parent
pub fn validate_block_regarding_parent(
    parent: &BlockLocked,
    block: &BlockLocked,
) -> Result<(), Error> {
    // TODO receipts_root if present

    // TODO  consensus
    //  * difficulty
    //  * gas_limit with basefee
    //  * timestamp
    //  * mix_hash & nonce PoW stuf
    //  * extra_data

    Ok(())
}

/// Checks
///     * If block_number if more then highest block: Block will be invalid as parent is unknown
///     * If we already know the block.
///     * If parent is known and if block number and block hash matches.
///
/// Returns parent block header  
pub fn validate_block_regarding_chain(
    block: &BlockLocked,
    _blockchain: usize,
) -> Result<HeaderLocked, Error> {
    let _id = block.header.hash();

    // Chain height needs to be more then current number.
    // if self.chain_heigh + 1 < block.number {
    //     return Err(eyre!("Block number is in far future"))
    // }

    // // Check if block is known.
    // if self.blocks.contains_key(&id) {
    //     return Err(eyre!("Block known"))
    // }

    // Check if parent is known.
    // if let Some(parent) = self.blocks.get(&block.parent_hash) {
    //     // And if block number is consistent.
    //     if parent.number + 1 != block.number {
    //         return Err(eyre!("There is gap with parent block number"))
    //     }
    // } else {
    //     return Err(eyre!("Parent block unknown"))
    // }

    Err(Error::HeaderFaileddd)
}

impl Blockchain {
    /// Create new blockchain.
    pub fn new(genesis: Header, config: Config) -> Self {
        Self { genesis, chain_heigh: 0, blocks: HashMap::new(), chain: HashMap::new(), config }
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
            return Err(eyre!("Block number is in far future"))
        }

        // Check if block is known.
        if self.blocks.contains_key(&id) {
            return Err(eyre!("Block known"))
        }

        // Check if parent is known.
        if let Some(parent) = self.blocks.get(&block.parent_hash) {
            // And if block number is consistent.
            if parent.number + 1 != block.number {
                return Err(eyre!("There is gap with parent block number"))
            }
        } else {
            return Err(eyre!("Parent block unknown"))
        }

        // Gas used needs to be less then gas limit. Gas used is going to be check after execution.
        if block.gas_used > block.gas_limit {
            return Err(eyre!("Block gas used is greater then gas limit"))
        }

        // check omners hash
        let omners_hash =
            crate::proofs::calculate_omners_root(block.omners.iter().map(|h| h.as_ref()));
        if block.header.ommers_hash != omners_hash {
            return Err(eyre!("Omner hash is different"))
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
}
