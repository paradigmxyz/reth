//! Collection of methods for block validation.

use alloy_consensus::{
    constants::MAXIMUM_EXTRA_DATA_SIZE, BlockHeader as _, EMPTY_OMMER_ROOT_HASH,
};
use alloy_eips::{calc_next_block_base_fee, eip4844::DATA_GAS_PER_BLOB, eip7840::BlobParams};
use reth_chainspec::{EthChainSpec, EthereumHardfork, EthereumHardforks};
use reth_consensus::ConsensusError;
use reth_primitives_traits::{
    Block, BlockBody, BlockHeader, GotExpected, SealedBlock, SealedHeader,
};

/// Gas used needs to be less than gas limit. Gas used is going to be checked after execution.
#[inline]
pub fn validate_header_gas<H: BlockHeader>(header: &H) -> Result<(), ConsensusError> {
    if header.gas_used() > header.gas_limit() {
        return Err(ConsensusError::HeaderGasUsedExceedsGasLimit {
            gas_used: header.gas_used(),
            gas_limit: header.gas_limit(),
        });
    }
    Ok(())
}

/// Ensure the EIP-1559 base fee is set if the London hardfork is active.
#[inline]
pub fn validate_header_base_fee<H: BlockHeader, ChainSpec: EthereumHardforks>(
    header: &H,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError> {
    if chain_spec.is_london_active_at_block(header.number()) && header.base_fee_per_gas().is_none() {
        return Err(ConsensusError::BaseFeeMissing);
    }
    Ok(())
}

/// Validate that withdrawals are present in Shanghai
///
/// See [EIP-4895]: https://eips.ethereum.org/EIPS/eip-4895
#[inline]
pub fn validate_shanghai_withdrawals<B: Block>(
    block: &SealedBlock<B>,
) -> Result<(), ConsensusError> {
    let withdrawals = block.body().withdrawals().ok_or(ConsensusError::BodyWithdrawalsMissing)?;
    let withdrawals_root = alloy_consensus::proofs::calculate_withdrawals_root(withdrawals);
    let header_withdrawals_root =
        block.withdrawals_root().ok_or(ConsensusError::WithdrawalsRootMissing)?;
    if withdrawals_root != *header_withdrawals_root {
        return Err(ConsensusError::BodyWithdrawalsRootDiff(
            GotExpected { got: withdrawals_root, expected: header_withdrawals_root }.into(),
        ));
    }
    Ok(())
}

/// Validate that blob gas is present in the block if Cancun is active.
///
/// See [EIP-4844]: Shard Blob Transactions
///
/// [EIP-4844]: https://eips.ethereum.org/EIPS/eip-4844
#[inline]
pub fn validate_cancun_gas<B: Block>(block: &SealedBlock<B>) -> Result<(), ConsensusError> {
    // Check that the blob gas used in the header matches the sum of the blob gas used by each
    // blob tx
    let header_blob_gas_used = block.blob_gas_used().ok_or(ConsensusError::BlobGasUsedMissing)?;
    let total_blob_gas = block.body().blob_gas_used();
    if total_blob_gas != header_blob_gas_used {
        return Err(ConsensusError::BlobGasUsedDiff(GotExpected {
            got: header_blob_gas_used,
            expected: total_blob_gas,
        }.into()));
    }
    Ok(())
}

/// Ensures the block response data matches the header.
///
/// This ensures the body response items match the header's hashes:
///   - ommer hash
///   - transaction root
///   - withdrawals root
pub fn validate_body_against_header<B, H>(body: &B, header: &H) -> Result<(), ConsensusError>
where
    B: BlockBody,
    H: BlockHeader,
{
    let ommers_hash = body.calculate_ommers_root();
    if Some(header.ommers_hash()) != ommers_hash {
        return Err(ConsensusError::BodyOmmersHashDiff(
            GotExpected {
                got: ommers_hash.unwrap_or(EMPTY_OMMER_ROOT_HASH),
                expected: header.ommers_hash(),
            }
            .into(),
        ));
    }

    let tx_root = body.calculate_tx_root();
    if header.transactions_root() != tx_root {
        return Err(ConsensusError::BodyTransactionRootDiff(
            GotExpected { got: tx_root, expected: header.transactions_root() }.into(),
        ));
    }

    match (header.withdrawals_root(), body.calculate_withdrawals_root()) {
        (Some(header_withdrawals_root), Some(withdrawals_root)) => {
            if withdrawals_root != header_withdrawals_root {
                return Err(ConsensusError::BodyWithdrawalsRootDiff(
                    GotExpected { got: withdrawals_root, expected: header_withdrawals_root }.into(),
                ));
            }
        }
        (None, None) => {
            // this is ok because we assume the fork is not active in this case
        }
        _ => return Err(ConsensusError::WithdrawalsRootUnexpected),
    }

    Ok(())
}

/// Validate a block without regard for state:
///
/// - Compares the ommer hash in the block header to the block body
/// - Compares the transactions root in the block header to the block body
/// - Pre-execution transaction validation
/// - (Optionally) Compares the receipts root in the block header to the block body
pub fn validate_block_pre_execution<B, ChainSpec>(
    block: &SealedBlock<B>,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError>
where
    B: Block,
    ChainSpec: EthereumHardforks,
{
    // Check ommers hash
    let ommers_hash = block.body().calculate_ommers_root();
    if Some(block.ommers_hash()) != ommers_hash {
        return Err(ConsensusError::BodyOmmersHashDiff(
            GotExpected {
                got: ommers_hash.unwrap_or(EMPTY_OMMER_ROOT_HASH),
                expected: block.ommers_hash(),
            }
            .into(),
        ));
    }

    // Check transaction root
    if let Err(error) = block.ensure_transaction_root_valid() {
        return Err(ConsensusError::BodyTransactionRootDiff(error.into()));
    }

    // EIP-4895: Beacon chain push withdrawals as operations
    if chain_spec.is_shanghai_active_at_timestamp(block.timestamp()) {
        validate_shanghai_withdrawals(block)?;
    }

    if chain_spec.is_cancun_active_at_timestamp(block.timestamp()) {
        validate_cancun_gas(block)?;
    }

    Ok(())
}

/// Validates that the EIP-4844 header fields exist and conform to the spec. This ensures that:
///
///  * `blob_gas_used` exists as a header field
///  * `excess_blob_gas` exists as a header field
///  * `parent_beacon_block_root` exists as a header field
///  * `blob_gas_used` is a multiple of `DATA_GAS_PER_BLOB`
///  * `excess_blob_gas` is a multiple of `DATA_GAS_PER_BLOB`
///
/// Note: This does not enforce any restrictions on `blob_gas_used`
pub fn validate_4844_header_standalone<H: BlockHeader>(header: &H) -> Result<(), ConsensusError> {
    let blob_gas_used = header.blob_gas_used().ok_or(ConsensusError::BlobGasUsedMissing)?;
    let excess_blob_gas = header.excess_blob_gas().ok_or(ConsensusError::ExcessBlobGasMissing)?;

    if header.parent_beacon_block_root().is_none() {
        return Err(ConsensusError::ParentBeaconBlockRootMissing);
    }

    if blob_gas_used % DATA_GAS_PER_BLOB != 0 {
        return Err(ConsensusError::BlobGasUsedNotMultipleOfBlobGasPerBlob {
            blob_gas_used,
            blob_gas_per_blob: DATA_GAS_PER_BLOB,
        });
    }

    // `excess_blob_gas` must also be a multiple of `DATA_GAS_PER_BLOB`. This will be checked later
    // (via `calc_excess_blob_gas`), but it doesn't hurt to catch the problem sooner.
    if excess_blob_gas % DATA_GAS_PER_BLOB != 0 {
        return Err(ConsensusError::ExcessBlobGasNotMultipleOfBlobGasPerBlob {
            excess_blob_gas,
            blob_gas_per_blob: DATA_GAS_PER_BLOB,
        });
    }

    Ok(())
}

/// Validates the header's extra data according to the beacon consensus rules.
///
/// From yellow paper: extraData: An arbitrary byte array containing data relevant to this block.
/// This must be 32 bytes or fewer; formally Hx.
#[inline]
pub fn validate_header_extra_data<H: BlockHeader>(header: &H) -> Result<(), ConsensusError> {
    let extra_data_len = header.extra_data().len();
    if extra_data_len > MAXIMUM_EXTRA_DATA_SIZE {
        return Err(ConsensusError::ExtraDataExceedsMax { len: extra_data_len });
    }
    Ok(())
}

/// Validates against the parent hash and number.
///
/// This function ensures that the header block number is sequential and that the hash of the parent
/// header matches the parent hash in the header.
#[inline]
pub fn validate_against_parent_hash_number<H: BlockHeader>(
    header: &H,
    parent: &SealedHeader<H>,
) -> Result<(), ConsensusError> {
    // Parent number is consistent.
    if parent.number() + 1 != header.number() {
        return Err(ConsensusError::ParentBlockNumberMismatch {
            parent_block_number: parent.number(),
            block_number: header.number(),
        });
    }

    // Parent hash is consistent.
    if parent.hash() != header.parent_hash() {
        return Err(ConsensusError::ParentHashMismatch(
            GotExpected { got: header.parent_hash(), expected: parent.hash() }.into(),
        ));
    }

    Ok(())
}
