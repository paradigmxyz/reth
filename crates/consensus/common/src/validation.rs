//! Collection of methods for block validation.

use reth_consensus::ConsensusError;
use reth_primitives::{
    constants::{
        eip4844::{DATA_GAS_PER_BLOB, MAX_DATA_GAS_PER_BLOCK},
        MAXIMUM_EXTRA_DATA_SIZE,
    },
    ChainSpec, GotExpected, Hardfork, Header, SealedBlock, SealedHeader,
};

/// Gas used needs to be less than gas limit. Gas used is going to be checked after execution.
#[inline]
pub fn validate_header_gas(header: &SealedHeader) -> Result<(), ConsensusError> {
    if header.gas_used > header.gas_limit {
        return Err(ConsensusError::HeaderGasUsedExceedsGasLimit {
            gas_used: header.gas_used,
            gas_limit: header.gas_limit,
        })
    }
    Ok(())
}

/// Ensure the EIP-1559 base fee is set if the London hardfork is active.
#[inline]
pub fn validate_header_base_fee(
    header: &SealedHeader,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError> {
    if chain_spec.fork(Hardfork::London).active_at_block(header.number) &&
        header.base_fee_per_gas.is_none()
    {
        return Err(ConsensusError::BaseFeeMissing)
    }
    Ok(())
}

/// Validates that the EIP-4844 header fields exist and conform to the spec. This ensures that:
///
///  * `blob_gas_used` exists as a header field
///  * `excess_blob_gas` exists as a header field
///  * `parent_beacon_block_root` exists as a header field
///  * `blob_gas_used` is less than or equal to `MAX_DATA_GAS_PER_BLOCK`
///  * `blob_gas_used` is a multiple of `DATA_GAS_PER_BLOB`
///  * `excess_blob_gas` is a multiple of `DATA_GAS_PER_BLOB`
pub fn validate_4844_header_standalone(header: &SealedHeader) -> Result<(), ConsensusError> {
    let blob_gas_used = header.blob_gas_used.ok_or(ConsensusError::BlobGasUsedMissing)?;
    let excess_blob_gas = header.excess_blob_gas.ok_or(ConsensusError::ExcessBlobGasMissing)?;

    if header.parent_beacon_block_root.is_none() {
        return Err(ConsensusError::ParentBeaconBlockRootMissing)
    }

    if blob_gas_used > MAX_DATA_GAS_PER_BLOCK {
        return Err(ConsensusError::BlobGasUsedExceedsMaxBlobGasPerBlock {
            blob_gas_used,
            max_blob_gas_per_block: MAX_DATA_GAS_PER_BLOCK,
        })
    }

    if blob_gas_used % DATA_GAS_PER_BLOB != 0 {
        return Err(ConsensusError::BlobGasUsedNotMultipleOfBlobGasPerBlob {
            blob_gas_used,
            blob_gas_per_blob: DATA_GAS_PER_BLOB,
        })
    }

    // `excess_blob_gas` must also be a multiple of `DATA_GAS_PER_BLOB`. This will be checked later
    // (via `calculate_excess_blob_gas`), but it doesn't hurt to catch the problem sooner.
    if excess_blob_gas % DATA_GAS_PER_BLOB != 0 {
        return Err(ConsensusError::ExcessBlobGasNotMultipleOfBlobGasPerBlob {
            excess_blob_gas,
            blob_gas_per_blob: DATA_GAS_PER_BLOB,
        })
    }

    Ok(())
}

/// Validates the header's extradata according to the beacon consensus rules.
///
/// From yellow paper: extraData: An arbitrary byte array containing data relevant to this block.
/// This must be 32 bytes or fewer; formally Hx.
#[inline]
pub fn validate_header_extradata(header: &Header) -> Result<(), ConsensusError> {
    if header.extra_data.len() > MAXIMUM_EXTRA_DATA_SIZE {
        Err(ConsensusError::ExtraDataExceedsMax { len: header.extra_data.len() })
    } else {
        Ok(())
    }
}

/// Validates the block's transaction root.
pub fn validate_transaction_root(block: &SealedBlock) -> Result<(), ConsensusError> {
    if let Err(error) = block.ensure_transaction_root_valid() {
        return Err(ConsensusError::BodyTransactionRootDiff(error.into()))
    }
    Ok(())
}

/// Validates the block's ommer hash.
pub fn validate_ommer_hash(block: &SealedBlock) -> Result<(), ConsensusError> {
    let ommers_hash = reth_primitives::proofs::calculate_ommers_root(&block.ommers);
    if block.header.ommers_hash != ommers_hash {
        return Err(ConsensusError::BodyOmmersHashDiff(
            GotExpected { got: ommers_hash, expected: block.header.ommers_hash }.into(),
        ))
    }
    Ok(())
}

/// Validates against the parent hash and number.
///
/// This function ensures that the header block number is sequential and that the hash of the parent
/// header matches the parent hash in the header.
#[inline]
pub fn validate_against_parent_hash_number(
    header: &SealedHeader,
    parent: &SealedHeader,
) -> Result<(), ConsensusError> {
    // Parent number is consistent.
    if parent.number + 1 != header.number {
        return Err(ConsensusError::ParentBlockNumberMismatch {
            parent_block_number: parent.number,
            block_number: header.number,
        })
    }

    if parent.hash() != header.parent_hash {
        return Err(ConsensusError::ParentHashMismatch(
            GotExpected { got: header.parent_hash, expected: parent.hash() }.into(),
        ))
    }

    Ok(())
}

/// Validates the base fee against the parent and EIP-1559 rules.
#[inline]
pub fn validate_against_parent_eip1559_base_fee(
    header: &SealedHeader,
    parent: &SealedHeader,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError> {
    if chain_spec.fork(Hardfork::London).active_at_block(header.number) {
        let base_fee = header.base_fee_per_gas.ok_or(ConsensusError::BaseFeeMissing)?;

        let expected_base_fee =
            if chain_spec.fork(Hardfork::London).transitions_at_block(header.number) {
                reth_primitives::constants::EIP1559_INITIAL_BASE_FEE
            } else {
                // This BaseFeeMissing will not happen as previous blocks are checked to have
                // them.
                parent
                    .next_block_base_fee(chain_spec.base_fee_params_at_timestamp(header.timestamp))
                    .ok_or(ConsensusError::BaseFeeMissing)?
            };
        if expected_base_fee != base_fee {
            return Err(ConsensusError::BaseFeeDiff(GotExpected {
                expected: expected_base_fee,
                got: base_fee,
            }))
        }
    }

    Ok(())
}

/// Validates the timestamp against the parent to make sure it is in the past.
#[inline]
pub fn validate_against_parent_timestamp(
    header: &SealedHeader,
    parent: &SealedHeader,
) -> Result<(), ConsensusError> {
    if header.is_timestamp_in_past(parent.timestamp) {
        return Err(ConsensusError::TimestampIsInPast {
            parent_timestamp: parent.timestamp,
            timestamp: header.timestamp,
        })
    }
    Ok(())
}
