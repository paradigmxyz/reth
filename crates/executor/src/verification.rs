//! ALl functions for verification of block
use crate::{config, Config};
use auto_impl::auto_impl;
use reth_primitives::{BlockHash, BlockLocked, BlockNumber, Header, HeaderLocked, H256};
use std::time::SystemTime;

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
    BlockKnown { hash: BlockHash, number: BlockNumber },
    #[error("Block parent [hash:{hash:?}] is not known")]
    ParentUnknown { hash: BlockHash },
    #[error("Block number {block_number:?} is missmatch with parent block number {parent_block_number:?}")]
    ParentBlockNumberMissmatch { parent_block_number: BlockNumber, block_number: BlockNumber },
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
    #[error("Base fee missing")]
    BaseFeeMissing,
    #[error("Block base fee ({got:?}) is different then expected: ({expected:?})")]
    BaseFeeDiff { expected: u64, got: u64 },
}

/// Validate header standalone
pub fn validate_header_standalone(
    header: &HeaderLocked,
    config: &config::Config,
) -> Result<(), Error> {
    // Gas used needs to be less then gas limit. Gas used is going to be check after execution.
    if header.gas_used > header.gas_limit {
        return Err(Error::HeaderGasUsedExceedsGasLimit {
            gas_used: header.gas_used,
            gas_limit: header.gas_limit,
        })
    }

    // Check if timestamp is in future. Clock can drift but this can be consensus issue.
    let present_timestamp =
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
    if header.timestamp > present_timestamp {
        return Err(Error::TimestampIsInFuture { timestamp: header.timestamp, present_timestamp })
    }

    // Check if base fee is set.
    if config.paris_hard_fork_block >= header.number && header.base_fee_per_gas.is_some() {
        return Err(Error::BaseFeeMissing)
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

/// Calculate base fee for next block. EIP-1559 spec
pub fn calculate_next_block_base_fee(gas_used: u64, gas_limit: u64, base_fee: u64) -> u64 {
    let gas_target = gas_limit / config::EIP1559_ELASTICITY_MULTIPLIER;

    if gas_used == gas_target {
        return base_fee
    }
    if gas_used > gas_target {
        let gas_used_delta = gas_used - gas_target;
        let base_fee_delta = std::cmp::max(
            1,
            base_fee as u128 * gas_used_delta as u128 /
                gas_target as u128 /
                config::EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR as u128,
        );
        base_fee + (base_fee_delta as u64)
    } else {
        let gas_used_delta = gas_target - gas_used;
        let base_fee_per_gas_delta = base_fee as u128 * gas_used_delta as u128 /
            gas_target as u128 /
            config::EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR as u128;

        base_fee.saturating_sub(base_fee_per_gas_delta as u64)
    }
}

/// Validate block in regards to parent
pub fn validate_header_regarding_parent(
    parent: &HeaderLocked,
    child: &HeaderLocked,
    config: &config::Config,
) -> Result<(), Error> {
    // Parent number is consistent.
    if parent.number + 1 != child.number {
        return Err(Error::ParentBlockNumberMissmatch {
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

    // difficulty check is done by consensus.
    if config.paris_hard_fork_block > child.number {
        // TODO how this needs to be checked? As ice age did increment it by some formula
    }

    let mut parent_gas_limit = parent.gas_limit;

    // By consensus, gas_limit is multiplied by elasticity (*2) on
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

    // EIP-1559 check base fee
    if child.number >= config.london_hard_fork_block {
        let base_fee = child.base_fee_per_gas.ok_or(Error::BaseFeeMissing)?;

        let expected_base_fee = if config.london_hard_fork_block == child.number {
            config::EIP1559_INITIAL_BASE_FEE
        } else {
            // This BaseFeeMissing will not happen as previous blocks are checked to have them.
            calculate_next_block_base_fee(
                parent.gas_used,
                parent.gas_limit,
                parent.base_fee_per_gas.ok_or(Error::BaseFeeMissing)?,
            )
        };
        if expected_base_fee != base_fee {
            return Err(Error::BaseFeeDiff { expected: expected_base_fee, got: base_fee })
        }
    }

    // TODO Consensus checks for:
    //  * mix_hash & nonce PoW stuf
    //  * extra_data

    Ok(())
}

/// Provider needs for verification of block agains the chain
///
/// TODO wrap all function around `Result` as this can trigger internal db error.
/// I didn't know what error to put (Probably needs to be database) so i left it for later as it
/// will be easy to integrate
#[auto_impl(&)]
pub trait BlockhainProvider {
    /// Check if block is known
    fn is_known(&self, block_hash: &BlockHash) -> bool;

    /// Get header by block hash
    fn header(&self, block_number: &BlockHash) -> Option<Header>;
}

/// Validate block in regards to chain (parent)
///
/// Checks:
///  If we already know the block.
///  If parent is known
///
/// Returns parent block header  
pub fn validate_block_regarding_chain<PROV: BlockhainProvider>(
    block: &BlockLocked,
    provider: PROV,
) -> Result<HeaderLocked, Error> {
    let hash = block.header.hash();

    // Check if block is known.
    if provider.is_known(&hash) {
        return Err(Error::BlockKnown { hash, number: block.header.number })
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
    config: &Config,
) -> Result<(), Error> {
    validate_header_standalone(&block.header, config)?;
    validate_block_standalone(&block)?;
    let parent = validate_block_regarding_chain(block, &provider)?;
    validate_header_regarding_parent(&parent, &block.header, config)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_base_fee_success() {
        let base_fee = [
            1000000000, 1000000000, 1000000000, 1072671875, 1059263476, 1049238967, 1049238967, 0,
            1, 2,
        ];
        let gas_used = [
            10000000, 10000000, 10000000, 9000000, 10001000, 0, 10000000, 10000000, 10000000,
            10000000,
        ];
        let gas_limit = [
            10000000, 12000000, 14000000, 10000000, 14000000, 2000000, 18000000, 18000000,
            18000000, 18000000,
        ];
        let next_base_fee = [
            1125000000, 1083333333, 1053571428, 1179939062, 1116028649, 918084097, 1063811730, 1,
            2, 3,
        ];

        for i in 0..base_fee.len() {
            assert_eq!(
                next_base_fee[i],
                calculate_next_block_base_fee(gas_used[i], gas_limit[i], base_fee[i])
            );
        }
    }
}
