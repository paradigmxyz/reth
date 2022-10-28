//! Blockchain is used as utility structure to chain multiple blocks into one block
use crate::{
    config,
    revm_wrap::{State, SubState},
    Config,
};
use auto_impl::auto_impl;
use eyre::eyre;
use reth_interfaces::executor::{BlockExecutor, ExecutorDb};
use reth_primitives::{
    Block, BlockHash, BlockID, BlockLocked, BlockNumber, Header, HeaderLocked, H256,
};
use std::{cmp::max, collections::HashMap, time::SystemTime};

/// Errors
#[allow(missing_docs)]
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Block used gas ({gas_used:?}) is greater then gas limit ({gas_limit:?})")]
    HeaderGasUsedExceedsGasLimit { gas_used: u64, gas_limit: u64 },
    #[error("Block ommner hash ({got:?}) is different then expected: ({expected:?})")]
    BodyOmmnersHashDiff { got: H256, expected: H256 },
    #[error("Block transaction root ({got:?}) is different then expected: ({expected:?})")]
    BodyTransactionRootDiff { got: H256, expected: H256 },
    #[error("Block receipts root ({got:?}) is different then expected: ({expected:?})")]
    BodyReceiptsRootDiff { got: H256, expected: H256 },
    #[error("Block with [hash:{hash:?},number: {number:}] is already known")]
    BlockUnknown { hash: BlockHash, number: BlockNumber },
    #[error("Block parent [hash:{hash:?}] is not known")]
    ParentUnknown { hash: BlockHash },
    #[error("Parent block number {parent_block_number:?} is not in chain of pending block number {block_number:?}")]
    ParentBlockNumber { parent_block_number: BlockNumber, block_number: BlockNumber },
    #[error(
        "Block timestamp {timestamp:?} is in past in comparison with parent timestamp {parent_timestamp:?}"
    )]
    TimestampIsInPast { parent_timestamp: u64, timestamp: u64 },
    #[error("Block timestamp {timestamp:?} is in future in comparison of our clock time {present_timestamp:?}")]
    TimestampIsInFuture { timestamp: u64, present_timestamp: u64 },
    // TODO make better error msg :)
    #[error("Child gas_limit {child_gas_limit:?} max increase is {parent_gas_limit}/1024")]
    GasLimitInvalidIncrease { parent_gas_limit: u64, child_gas_limit: u64 },
    #[error("Child gas_limit {child_gas_limit:?} max decrease is {parent_gas_limit}/1024")]
    GasLimitInvalidDecrease { parent_gas_limit: u64, child_gas_limit: u64 },
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

    // TODO Check if this needs to be configurable
    // Check if timestamp is in future. Clock can drift but this can be consensus issue.
    let present_timestamp =
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
    if header.timestamp > present_timestamp {
        return Err(Error::TimestampIsInFuture { timestamp: header.timestamp, present_timestamp })
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

/// Validate block in regards to parent
pub fn validate_header_regarding_parent(
    parent: &HeaderLocked,
    child: &HeaderLocked,
    config: &Config,
) -> Result<(), Error> {
    // Parent number is consistent.
    if parent.number + 1 != child.number {
        return Err(Error::ParentBlockNumber {
            parent_block_number: parent.number,
            block_number: child.number,
        })
    }

    // timestamp in past check
    if child.timestamp < parent.timestamp {
        return Err(Error::TimestampIsInPast {
            parent_timestamp: parent.timestamp,
            timestamp: child.timestamp,
        })
    }

    // TODO add configurable diffuculty check. Disable after The Merge.
    // difficulty check is done by consensus

    let mut parent_gas_limit = parent.gas_limit;

    // By consensus gas_limit is multiplied by elasticity (*2) to make a full block on
    // on exact block that hardfork happens.
    if config.london_hard_fork_block == child.number {
        parent_gas_limit = parent.gas_limit * config::EIP1559_ELASTICITY_MULTIPLIER;
    }

    // Check gas limit, max diff between child/parent gas_limit should be  max_diff=parent_gas/1024
    if child.gas_limit > parent_gas_limit {
        if child.gas_limit - parent_gas_limit >= parent_gas_limit / 1024 {
            return Err(Error::GasLimitInvalidIncrease {
                parent_gas_limit,
                child_gas_limit: child.gas_limit,
            })
        }
    } else {
        if parent_gas_limit - child.gas_limit >= parent_gas_limit / 1024 {
            return Err(Error::GasLimitInvalidDecrease {
                parent_gas_limit,
                child_gas_limit: child.gas_limit,
            })
        }
    }

    // basefee

    // Consensus:
    //  * mix_hash & nonce PoW stuf
    //  * extra_data

    Ok(())
}

/// Provider needs for verification of block agains the chain
/// TODO wrap all function around `Result` as this can trigger internal db error.
/// I didn't know what error to put (Probably needs to be database) so i left it for later as it
/// will be easy to integrate
#[auto_impl(&)]
pub trait BlockhainProvider {
    /// Check if block is known
    fn is_known(&self, block_hash: &BlockHash) -> bool;

    /// Get header by block hash
    fn header(&self, block_number: &BlockHash) -> Option<Header>;

    fn config(&self) -> &Config;
}

/// Checks
///     * If we already know the block.
///     * If parent is known
///
/// Returns parent block header  
pub fn validate_block_regarding_chain<PROV: BlockhainProvider>(
    block: &BlockLocked,
    provider: PROV,
) -> Result<HeaderLocked, Error> {
    let hash = block.header.hash();

    // Check if block is known.
    if provider.is_known(&hash) {
        return Err(Error::BlockUnknown { hash, number: block.header.number })
    }

    // Check if parent is known.
    let parent = provider
        .header(&block.parent_hash)
        .ok_or(Error::ParentUnknown { hash: block.parent_hash })?;

    // Return parent header.
    Ok(parent.lock())
}

/// Full validation of block before execution.
pub fn full_validation<PROV: BlockhainProvider>(
    block: &BlockLocked,
    provider: PROV,
) -> Result<(), Error> {
    validate_header_standalone(&block.header)?;
    validate_block_standalone(&block)?;
    let parent = validate_block_regarding_chain(block, &provider)?;
    let config = provider.config();
    validate_header_regarding_parent(&parent, &block.header, config)?;

    Ok(())
}

/// Before execution do verification on header and body if it can be included
/// inside blockchain. TODO see where this should be placed.
pub async fn pre_verification(block: BlockLocked) -> eyre::Result<()> {
    let id = block.header.hash();

    // Gas used needs to be less then gas limit. Gas used is going to be check after execution.
    if block.gas_used > block.gas_limit {
        return Err(eyre!("Block gas used is greater then gas limit"))
    }

    // check omners hash
    let omners_hash = crate::proofs::calculate_omners_root(block.omners.iter().map(|h| h.as_ref()));
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
pub async fn post_verification(block: BlockLocked) -> eyre::Result<()> {
    // TODO block.state_root;
    // block.logs_bloom;

    // block.gas_used & if limit is hit
    Ok(())
}
