//! Collection of methods for block validation.
use reth_interfaces::consensus::Error;
use reth_primitives::{
    constants::{EIP1559_ELASTICITY_MULTIPLIER, EIP1559_INITIAL_BASE_FEE, GAS_LIMIT_BOUND_DIVISOR},
    ChainSpec, Hardfork, SealedHeader, EMPTY_OMMER_ROOT, U256,
};
use std::time::SystemTime;

use super::calculate_next_block_base_fee;

/// Validate header standalone
pub fn validate_header_standalone(
    header: &SealedHeader,
    chain_spec: &ChainSpec,
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

    // From yellow paper: extraData: An arbitrary byte array containing data
    // relevant to this block. This must be 32 bytes or fewer; formally Hx.
    if header.extra_data.len() > 32 {
        return Err(Error::ExtraDataExceedsMax { len: header.extra_data.len() })
    }

    // Check if base fee is set.
    if chain_spec.fork_active(Hardfork::London, header.number) && header.base_fee_per_gas.is_none()
    {
        return Err(Error::BaseFeeMissing)
    }

    // EIP-3675: Upgrade consensus to Proof-of-Stake:
    // https://eips.ethereum.org/EIPS/eip-3675#replacing-difficulty-with-0
    if Some(header.number) >= chain_spec.paris_status().block_number() {
        if header.difficulty != U256::ZERO {
            return Err(Error::TheMergeDifficultyIsNotZero)
        }

        if header.nonce != 0 {
            return Err(Error::TheMergeNonceIsNotZero)
        }

        if header.ommers_hash != EMPTY_OMMER_ROOT {
            return Err(Error::TheMergeOmmerRootIsNotEmpty)
        }

        // mixHash is used instead of difficulty inside EVM
        // https://eips.ethereum.org/EIPS/eip-4399#using-mixhash-field-instead-of-difficulty
    }

    Ok(())
}

/// Validate block in regards to parent
pub fn validate_header_regarding_parent(
    parent: &SealedHeader,
    child: &SealedHeader,
    chain_spec: &ChainSpec,
) -> Result<(), Error> {
    // Parent number is consistent.
    if parent.number + 1 != child.number {
        return Err(Error::ParentBlockNumberMismatch {
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
    if chain_spec.paris_status().block_number() > Some(child.number) {
        // TODO how this needs to be checked? As ice age did increment it by some formula
    }

    // Validate gas limit increase/decrease.
    validate_gas_limit_difference(child, parent, chain_spec)?;

    // EIP-1559 check base fee
    if chain_spec.fork_active(Hardfork::London, child.number) {
        let base_fee = child.base_fee_per_gas.ok_or(Error::BaseFeeMissing)?;

        let expected_base_fee = if chain_spec.fork_block(Hardfork::London) == Some(child.number) {
            EIP1559_INITIAL_BASE_FEE
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

    Ok(())
}

/// Verify the header gas limit according increase/decrease
/// in relation to the parent gas limit.
/// https://github.com/ethereum/go-ethereum/blob/d0a4989a8def7e6bad182d1513e8d4a093c1672d/consensus/misc/gaslimit.go#L28
pub fn validate_gas_limit_difference(
    child: &SealedHeader,
    parent: &SealedHeader,
    chain_spec: &ChainSpec,
) -> Result<(), Error> {
    let mut parent_gas_limit = parent.gas_limit;

    // By consensus, gas_limit is multiplied by elasticity (*2) on
    // on exact block that hardfork happens.
    if chain_spec.fork_block(Hardfork::London) == Some(child.number) {
        parent_gas_limit = parent.gas_limit * EIP1559_ELASTICITY_MULTIPLIER;
    }

    // Check gas limit, max diff between child/parent gas_limit should be  max_diff=parent_gas/1024
    if child.gas_limit > parent_gas_limit {
        if child.gas_limit - parent_gas_limit >= parent_gas_limit / GAS_LIMIT_BOUND_DIVISOR {
            return Err(Error::GasLimitInvalidIncrease {
                parent_gas_limit,
                child_gas_limit: child.gas_limit,
            })
        }
    } else if parent_gas_limit - child.gas_limit >= parent_gas_limit / GAS_LIMIT_BOUND_DIVISOR {
        return Err(Error::GasLimitInvalidDecrease {
            parent_gas_limit,
            child_gas_limit: child.gas_limit,
        })
    }

    Ok(())
}
