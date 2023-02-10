//! Collection of methods for header validation.
use super::calculate_next_block_base_fee;
use reth_interfaces::consensus::Error;
use reth_primitives::{
    constants::{EIP1559_ELASTICITY_MULTIPLIER, EIP1559_INITIAL_BASE_FEE, GAS_LIMIT_BOUND_DIVISOR},
    ChainSpec, Hardfork, SealedHeader,
};
use std::time::SystemTime;

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
    if chain_spec.fork(Hardfork::London).active_at_block(header.number) &&
        header.base_fee_per_gas.is_none()
    {
        return Err(Error::BaseFeeMissing)
    }

    Ok(())
}

/// Validate block in regards to parent
pub fn validate_header_regarding_parent(
    parent: &SealedHeader,
    child: &SealedHeader,
    chain_spec: &ChainSpec,
) -> Result<(), Error> {
    // Check that the parent number is consistent.
    if parent.number + 1 != child.number {
        return Err(Error::ParentBlockNumberMismatch {
            parent_block_number: parent.number,
            block_number: child.number,
        })
    }

    // Check that the parent hash is consistent.
    if parent.hash() != child.parent_hash {
        return Err(Error::ParentHashMismatch {
            block_number: child.number,
            expected: child.parent_hash,
            received: parent.hash(),
        })
    }

    // Check that the child timestamp is not in the past
    if child.timestamp < parent.timestamp {
        return Err(Error::TimestampIsInPast {
            parent_timestamp: parent.timestamp,
            timestamp: child.timestamp,
        })
    }

    // difficulty check is done by consensus.
    // TODO(onbjerg): Unsure what the check here is supposed to be, but it should be moved to
    // [BeaconConsensus].
    // if chain_spec.paris_status().block_number() > Some(child.number) {
    //    // TODO how this needs to be checked? As ice age did increment it by some formula
    // }

    // Validate gas limit increase/decrease.
    validate_gas_limit_difference(child, parent, chain_spec)?;

    // EIP-1559 check base fee
    validate_eip_1559_base_fee(child, parent, chain_spec)?;

    Ok(())
}

/// Validate that the number, parent hash and timestamp fields are consistent
/// regarding the parent header.
pub fn validate_header_consistency(
    parent: &SealedHeader,
    child: &SealedHeader,
) -> Result<(), Error> {
    // Check that the parent number is consistent.
    if parent.number + 1 != child.number {
        return Err(Error::ParentBlockNumberMismatch {
            parent_block_number: parent.number,
            block_number: child.number,
        })
    }

    // Check that the parent hash is consistent.
    if parent.hash() != child.parent_hash {
        return Err(Error::ParentHashMismatch {
            block_number: child.number,
            expected: child.parent_hash,
            received: parent.hash(),
        })
    }

    // Check that the child timestamp is not in the past
    if child.timestamp < parent.timestamp {
        return Err(Error::TimestampIsInPast {
            parent_timestamp: parent.timestamp,
            timestamp: child.timestamp,
        })
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
    if chain_spec.fork(Hardfork::London).transitions_at_block(child.number) {
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

/// Validate EIP-1559 base fee increase/decrease.
pub fn validate_eip_1559_base_fee(
    child: &SealedHeader,
    parent: &SealedHeader,
    chain_spec: &ChainSpec,
) -> Result<(), Error> {
    if chain_spec.fork(Hardfork::London).active_at_block(child.number) {
        let base_fee = child.base_fee_per_gas.ok_or(Error::BaseFeeMissing)?;

        let expected_base_fee =
            if chain_spec.fork(Hardfork::London).transitions_at_block(child.number) {
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
        Ok(())
    } else if child.base_fee_per_gas.is_some() {
        Err(Error::UnexpectedBaseFee)
    } else {
        Ok(())
    }
}

/// Methods for validating clique headers
pub mod clique {
    use super::*;
    use crate::clique::{
        snapshot::Snapshot,
        utils::{is_checkpoint_block, recover_header_signer},
    };
    use reth_interfaces::consensus::{CliqueError, Error};
    use reth_primitives::{
        constants::clique::*, Address, Bytes, ChainSpec, CliqueConfig, SealedHeader,
        EMPTY_OMMER_ROOT, H64, U256,
    };
    use std::time::SystemTime;

    /// Validate header according to clique consensus rules.
    pub fn validate_header_standalone(
        header: &SealedHeader,
        config: &CliqueConfig,
    ) -> Result<(), Error> {
        // Gas used needs to be less then gas limit. Gas used is going to be check after execution.
        if header.gas_used > header.gas_limit {
            return Err(Error::HeaderGasUsedExceedsGasLimit {
                gas_used: header.gas_used,
                gas_limit: header.gas_limit,
            })
        }

        // Check if timestamp is in future.
        let present_timestamp =
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        if header.timestamp > present_timestamp {
            return Err(Error::TimestampIsInFuture {
                timestamp: header.timestamp,
                present_timestamp,
            })
        }

        let is_checkpoint = is_checkpoint_block(config, header.number);

        // Checkpoint blocks need to enforce zero beneficiary
        if is_checkpoint && !header.beneficiary.is_zero() {
            return Err(CliqueError::CheckpointBeneficiary { beneficiary: header.beneficiary }.into())
        }

        // Nonces must be 0x00..0 or 0xff..f, zeroes enforced on checkpoints
        let nonce = H64::from_low_u64_ne(header.nonce);
        if is_checkpoint && nonce.as_bytes() != NONCE_DROP_VOTE {
            return Err(CliqueError::CheckpointVote { nonce: header.nonce }.into())
        } else if nonce.as_bytes() != NONCE_AUTH_VOTE {
            return Err(CliqueError::InvalidVote { nonce: header.nonce }.into())
        }

        // Check that the extra-data contains both the vanity and signature
        let extra_data_len = header.extra_data.len();
        if extra_data_len < EXTRA_VANITY {
            return Err(CliqueError::MissingVanity { extra_data: header.extra_data.clone() }.into())
        } else if extra_data_len < EXTRA_VANITY + EXTRA_SEAL {
            return Err(
                CliqueError::MissingSignature { extra_data: header.extra_data.clone() }.into()
            )
        }

        // Ensure that the extra-data contains a signer list on checkpoint, but none otherwise
        // Safe subtraction because of the check above.
        let signers_bytes_len = extra_data_len - EXTRA_VANITY - EXTRA_SEAL;
        if is_checkpoint && signers_bytes_len != 0 {
            return Err(CliqueError::ExtraSignerList { extra_data: header.extra_data.clone() }.into())
        } else if signers_bytes_len % Address::len_bytes() != 0 {
            return Err(
                CliqueError::CheckpointSigners { extra_data: header.extra_data.clone() }.into()
            )
        }

        // Ensure that the mix digest is zero as we don't have fork protection currently
        if !header.mix_hash.is_zero() {
            return Err(CliqueError::NonZeroMixHash { mix_hash: header.mix_hash }.into())
        }

        // Ensure that the block doesn't contain any uncles which are meaningless in PoA
        if header.ommers_hash != EMPTY_OMMER_ROOT {
            return Err(Error::BodyOmmersHashDiff {
                got: header.ommers_hash,
                expected: EMPTY_OMMER_ROOT,
            })
        }

        // Ensure that the block's difficulty is meaningful (may not be correct at this point)
        if header.number > 0 &&
            header.difficulty != U256::from(DIFF_INTURN) &&
            header.difficulty != U256::from(DIFF_NOTURN)
        {
            return Err(CliqueError::Difficulty { difficulty: header.difficulty }.into())
        }

        Ok(())
    }

    /// Validate header regarding to its parent according to clique consensus.
    pub fn validate_header_regarding_parent(
        parent: &SealedHeader,
        child: &SealedHeader,
        config: &CliqueConfig,
        chain_spec: &ChainSpec,
    ) -> Result<(), Error> {
        // Validate that at least `period` seconds have passed since last block.
        let expected_at_least = parent.timestamp + config.period;
        if child.timestamp < expected_at_least {
            return Err(
                CliqueError::Timestamp { expected_at_least, received: child.timestamp }.into()
            )
        }

        // Validate child/parent consistency.
        validate_header_consistency(parent, child)?;

        // Validate gas limit increase/decrease.
        validate_gas_limit_difference(child, parent, chain_spec)?;

        // EIP-1559 check base fee
        validate_eip_1559_base_fee(child, parent, chain_spec)?;

        Ok(())
    }

    /// Validate header regarding snapshot according to clique consensus.
    pub fn validate_header_regarding_snapshot(
        header: &SealedHeader,
        snapshot: &Snapshot,
        config: &CliqueConfig,
    ) -> Result<(), Error> {
        // If the block is a checkpoint block, verify the signer list
        let is_checkpoint = is_checkpoint_block(config, header.number);
        if is_checkpoint {
            let signer_bytes = snapshot.signers_bytes();
            let extra_suffix = header.extra_data.len() - EXTRA_SEAL;
            let extra_data_bytes = Bytes::from(&header.extra_data[EXTRA_VANITY..extra_suffix]);
            if extra_data_bytes != signer_bytes {
                return Err(
                    CliqueError::CheckpointSigners { extra_data: header.extra_data.clone() }.into()
                )
            }
        }

        let signer = recover_header_signer(header)?;

        // Verify that the header signer is authorized
        if !snapshot.is_authorized_signer(&signer) {
            return Err(CliqueError::UnauthorizedSigner { signer }.into())
        }

        if let Some(recent_block) = snapshot.find_recent_signer_block(&signer) {
            // Signer is among recents, only fail if the current block doesn't shift it out
            let limit = snapshot.recents_cache_limit();
            if recent_block > header.number - limit {
                return Err(CliqueError::RecentSigner { signer }.into())
            }
        }

        // Ensure that the difficulty corresponds to the turn-ness of the signer
        let inturn = snapshot
            .is_signer_inturn(&signer, header.number)
            .ok_or(CliqueError::UnauthorizedSigner { signer })?;
        if (inturn && header.difficulty != U256::from(DIFF_INTURN)) ||
            header.difficulty != U256::from(DIFF_NOTURN)
        {
            return Err(CliqueError::Difficulty { difficulty: header.difficulty }.into())
        }

        Ok(())
    }
}
