//! Collection of methods for block validation.

use alloy_consensus::{constants::MAXIMUM_EXTRA_DATA_SIZE, BlockHeader, EMPTY_OMMER_ROOT_HASH};
use alloy_eips::{calc_next_block_base_fee, eip4844::DATA_GAS_PER_BLOB, eip7840::BlobParams};
use reth_chainspec::{EthChainSpec, EthereumHardfork, EthereumHardforks};
use reth_consensus::ConsensusError;
use reth_primitives::SealedBlock;
use reth_primitives_traits::{BlockBody, GotExpected, SealedHeader};

/// Gas used needs to be less than gas limit. Gas used is going to be checked after execution.
#[inline]
pub fn validate_header_gas<H: BlockHeader>(header: &H) -> Result<(), ConsensusError> {
    if header.gas_used() > header.gas_limit() {
        return Err(ConsensusError::HeaderGasUsedExceedsGasLimit {
            gas_used: header.gas_used(),
            gas_limit: header.gas_limit(),
        })
    }
    Ok(())
}

/// Ensure the EIP-1559 base fee is set if the London hardfork is active.
#[inline]
pub fn validate_header_base_fee<H: BlockHeader, ChainSpec: EthereumHardforks>(
    header: &H,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError> {
    if chain_spec.is_fork_active_at_block(EthereumHardfork::London, header.number()) &&
        header.base_fee_per_gas().is_none()
    {
        return Err(ConsensusError::BaseFeeMissing)
    }
    Ok(())
}

/// Validate that withdrawals are present in Shanghai
///
/// See [EIP-4895]: Beacon chain push withdrawals as operations
///
/// [EIP-4895]: https://eips.ethereum.org/EIPS/eip-4895
#[inline]
pub fn validate_shanghai_withdrawals<H: BlockHeader, B: BlockBody>(
    block: &SealedBlock<H, B>,
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
pub fn validate_cancun_gas<H: BlockHeader, B: BlockBody>(
    block: &SealedBlock<H, B>,
) -> Result<(), ConsensusError> {
    // Check that the blob gas used in the header matches the sum of the blob gas used by each
    // blob tx
    let header_blob_gas_used =
        block.header().blob_gas_used().ok_or(ConsensusError::BlobGasUsedMissing)?;
    let total_blob_gas = block.body().blob_gas_used();
    if total_blob_gas != header_blob_gas_used {
        return Err(ConsensusError::BlobGasUsedDiff(GotExpected {
            got: header_blob_gas_used,
            expected: total_blob_gas,
        }));
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
        ))
    }

    let tx_root = body.calculate_tx_root();
    if header.transactions_root() != tx_root {
        return Err(ConsensusError::BodyTransactionRootDiff(
            GotExpected { got: tx_root, expected: header.transactions_root() }.into(),
        ))
    }

    match (header.withdrawals_root(), body.calculate_withdrawals_root()) {
        (Some(header_withdrawals_root), Some(withdrawals_root)) => {
            if withdrawals_root != header_withdrawals_root {
                return Err(ConsensusError::BodyWithdrawalsRootDiff(
                    GotExpected { got: withdrawals_root, expected: header_withdrawals_root }.into(),
                ))
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
pub fn validate_block_pre_execution<H, B, ChainSpec>(
    block: &SealedBlock<H, B>,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError>
where
    H: BlockHeader,
    B: BlockBody,
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
        ))
    }

    // Check transaction root
    if let Err(error) = block.ensure_transaction_root_valid() {
        return Err(ConsensusError::BodyTransactionRootDiff(error.into()))
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
        return Err(ConsensusError::ParentBeaconBlockRootMissing)
    }

    if blob_gas_used % DATA_GAS_PER_BLOB != 0 {
        return Err(ConsensusError::BlobGasUsedNotMultipleOfBlobGasPerBlob {
            blob_gas_used,
            blob_gas_per_blob: DATA_GAS_PER_BLOB,
        })
    }

    // `excess_blob_gas` must also be a multiple of `DATA_GAS_PER_BLOB`. This will be checked later
    // (via `calc_excess_blob_gas`), but it doesn't hurt to catch the problem sooner.
    if excess_blob_gas % DATA_GAS_PER_BLOB != 0 {
        return Err(ConsensusError::ExcessBlobGasNotMultipleOfBlobGasPerBlob {
            excess_blob_gas,
            blob_gas_per_blob: DATA_GAS_PER_BLOB,
        })
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
        Err(ConsensusError::ExtraDataExceedsMax { len: extra_data_len })
    } else {
        Ok(())
    }
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
        })
    }

    if parent.hash() != header.parent_hash() {
        return Err(ConsensusError::ParentHashMismatch(
            GotExpected { got: header.parent_hash(), expected: parent.hash() }.into(),
        ))
    }

    Ok(())
}

/// Validates the base fee against the parent and EIP-1559 rules.
#[inline]
pub fn validate_against_parent_eip1559_base_fee<
    H: BlockHeader,
    ChainSpec: EthChainSpec + EthereumHardforks,
>(
    header: &H,
    parent: &H,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError> {
    if chain_spec.fork(EthereumHardfork::London).active_at_block(header.number()) {
        let base_fee = header.base_fee_per_gas().ok_or(ConsensusError::BaseFeeMissing)?;

        let expected_base_fee =
            if chain_spec.fork(EthereumHardfork::London).transitions_at_block(header.number()) {
                alloy_eips::eip1559::INITIAL_BASE_FEE
            } else {
                // This BaseFeeMissing will not happen as previous blocks are checked to have
                // them.
                let base_fee = parent.base_fee_per_gas().ok_or(ConsensusError::BaseFeeMissing)?;
                calc_next_block_base_fee(
                    parent.gas_used(),
                    parent.gas_limit(),
                    base_fee,
                    chain_spec.base_fee_params_at_timestamp(header.timestamp()),
                )
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
pub fn validate_against_parent_timestamp<H: BlockHeader>(
    header: &H,
    parent: &H,
) -> Result<(), ConsensusError> {
    if header.timestamp() <= parent.timestamp() {
        return Err(ConsensusError::TimestampIsInPast {
            parent_timestamp: parent.timestamp(),
            timestamp: header.timestamp(),
        })
    }
    Ok(())
}

/// Validates that the EIP-4844 header fields are correct with respect to the parent block. This
/// ensures that the `blob_gas_used` and `excess_blob_gas` fields exist in the child header, and
/// that the `excess_blob_gas` field matches the expected `excess_blob_gas` calculated from the
/// parent header fields.
pub fn validate_against_parent_4844<H: BlockHeader>(
    header: &H,
    parent: &H,
    blob_params: BlobParams,
) -> Result<(), ConsensusError> {
    // From [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#header-extension):
    //
    // > For the first post-fork block, both parent.blob_gas_used and parent.excess_blob_gas
    // > are evaluated as 0.
    //
    // This means in the first post-fork block, calc_excess_blob_gas will return 0.
    let parent_blob_gas_used = parent.blob_gas_used().unwrap_or(0);
    let parent_excess_blob_gas = parent.excess_blob_gas().unwrap_or(0);

    if header.blob_gas_used().is_none() {
        return Err(ConsensusError::BlobGasUsedMissing)
    }
    let excess_blob_gas = header.excess_blob_gas().ok_or(ConsensusError::ExcessBlobGasMissing)?;

    let expected_excess_blob_gas =
        blob_params.next_block_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used);
    if expected_excess_blob_gas != excess_blob_gas {
        return Err(ConsensusError::ExcessBlobGasDiff {
            diff: GotExpected { got: excess_blob_gas, expected: expected_excess_blob_gas },
            parent_excess_blob_gas,
            parent_blob_gas_used,
        })
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Header, TxEip4844};
    use alloy_eips::eip4895::Withdrawals;
    use alloy_primitives::{Address, Bytes, PrimitiveSignature as Signature, U256};
    use rand::Rng;
    use reth_chainspec::ChainSpecBuilder;
    use reth_primitives::{proofs, BlockBody, Transaction, TransactionSigned};

    fn mock_blob_tx(nonce: u64, num_blobs: usize) -> TransactionSigned {
        let mut rng = rand::thread_rng();
        let request = Transaction::Eip4844(TxEip4844 {
            chain_id: 1u64,
            nonce,
            max_fee_per_gas: 0x28f000fff,
            max_priority_fee_per_gas: 0x28f000fff,
            max_fee_per_blob_gas: 0x7,
            gas_limit: 10,
            to: Address::default(),
            value: U256::from(3_u64),
            input: Bytes::from(vec![1, 2]),
            access_list: Default::default(),
            blob_versioned_hashes: std::iter::repeat_with(|| rng.gen()).take(num_blobs).collect(),
        });

        let signature = Signature::new(U256::default(), U256::default(), true);

        TransactionSigned::new_unhashed(request, signature)
    }

    #[test]
    fn cancun_block_incorrect_blob_gas_used() {
        let chain_spec = ChainSpecBuilder::mainnet().cancun_activated().build();

        // create a tx with 10 blobs
        let transaction = mock_blob_tx(1, 10);

        let header = Header {
            base_fee_per_gas: Some(1337),
            withdrawals_root: Some(proofs::calculate_withdrawals_root(&[])),
            blob_gas_used: Some(1),
            transactions_root: proofs::calculate_transaction_root(&[transaction.clone()]),
            ..Default::default()
        };
        let header = SealedHeader::seal(header);

        let body = BlockBody {
            transactions: vec![transaction],
            ommers: vec![],
            withdrawals: Some(Withdrawals::default()),
        };

        let block = SealedBlock::new(header, body);

        // 10 blobs times the blob gas per blob.
        let expected_blob_gas_used = 10 * DATA_GAS_PER_BLOB;

        // validate blob, it should fail blob gas used validation
        assert_eq!(
            validate_block_pre_execution(&block, &chain_spec),
            Err(ConsensusError::BlobGasUsedDiff(GotExpected {
                got: 1,
                expected: expected_blob_gas_used
            }))
        );
    }
}
