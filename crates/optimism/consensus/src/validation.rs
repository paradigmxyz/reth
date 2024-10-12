use crate::proof::calculate_receipt_root_optimism;
use alloy_primitives::{Bloom, B256};
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_consensus::ConsensusError;
use reth_primitives::{gas_spent_by_transactions, BlockWithSenders, GotExpected, Receipt};

/// Validate a block with regard to execution results:
///
/// - Compares the receipts root in the block header to the block body
/// - Compares the gas used in the block header to the actual gas usage after execution
pub fn validate_block_post_execution(
    block: &BlockWithSenders,
    chain_spec: &ChainSpec,
    receipts: &[Receipt],
) -> Result<(), ConsensusError> {
    // Before Byzantium, receipts contained state root that would mean that expensive
    // operation as hashing that is required for state root got calculated in every
    // transaction This was replaced with is_success flag.
    // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
    if chain_spec.is_byzantium_active_at_block(block.header.number) {
        if let Err(error) = verify_receipts(
            block.header.receipts_root,
            block.header.logs_bloom,
            receipts,
            chain_spec,
            block.timestamp,
        ) {
            tracing::debug!(%error, ?receipts, "receipts verification failed");
            return Err(error)
        }
    }

    // Check if gas used matches the value set in header.
    let cumulative_gas_used =
        receipts.last().map(|receipt| receipt.cumulative_gas_used).unwrap_or(0);
    if block.gas_used != cumulative_gas_used {
        return Err(ConsensusError::BlockGasUsed {
            gas: GotExpected { got: cumulative_gas_used, expected: block.gas_used },
            gas_spent_by_tx: gas_spent_by_transactions(receipts),
        })
    }

    Ok(())
}

/// Verify the calculated receipts root against the expected receipts root.
fn verify_receipts(
    expected_receipts_root: B256,
    expected_logs_bloom: Bloom,
    receipts: &[Receipt],
    chain_spec: &ChainSpec,
    timestamp: u64,
) -> Result<(), ConsensusError> {
    // Calculate receipts root.
    let receipts_with_bloom = receipts.iter().cloned().map(Receipt::with_bloom).collect::<Vec<_>>();
    let receipts_root =
        calculate_receipt_root_optimism(&receipts_with_bloom, chain_spec, timestamp);

    // Calculate header logs bloom.
    let logs_bloom = receipts_with_bloom.iter().fold(Bloom::ZERO, |bloom, r| bloom | r.bloom);

    compare_receipts_root_and_logs_bloom(
        receipts_root,
        logs_bloom,
        expected_receipts_root,
        expected_logs_bloom,
    )?;

    Ok(())
}

/// Compare the calculated receipts root with the expected receipts root, also compare
/// the calculated logs bloom with the expected logs bloom.
fn compare_receipts_root_and_logs_bloom(
    calculated_receipts_root: B256,
    calculated_logs_bloom: Bloom,
    expected_receipts_root: B256,
    expected_logs_bloom: Bloom,
) -> Result<(), ConsensusError> {
    if calculated_receipts_root != expected_receipts_root {
        return Err(ConsensusError::BodyReceiptRootDiff(
            GotExpected { got: calculated_receipts_root, expected: expected_receipts_root }.into(),
        ))
    }

    if calculated_logs_bloom != expected_logs_bloom {
        return Err(ConsensusError::BodyBloomLogDiff(
            GotExpected { got: calculated_logs_bloom, expected: expected_logs_bloom }.into(),
        ))
    }

    Ok(())
}
