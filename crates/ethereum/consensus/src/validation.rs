use alloy_primitives::{Bloom, B256};
use reth_chainspec::EthereumHardforks;
use reth_consensus::ConsensusError;
use reth_primitives::{gas_spent_by_transactions, BlockWithSenders, GotExpected, Receipt, Request};

/// Validate a block with regard to execution results:
///
/// - Compares the receipts root in the block header to the block body
/// - Compares the gas used in the block header to the actual gas usage after execution
pub fn validate_block_post_execution<ChainSpec: EthereumHardforks>(
    block: &BlockWithSenders,
    chain_spec: &ChainSpec,
    receipts: &[Receipt],
    requests: &[Request],
) -> Result<(), ConsensusError> {
    // Check if gas used matches the value set in header.
    let cumulative_gas_used =
        receipts.last().map(|receipt| receipt.cumulative_gas_used).unwrap_or(0);
    if block.gas_used != cumulative_gas_used {
        return Err(ConsensusError::BlockGasUsed {
            gas: GotExpected { got: cumulative_gas_used, expected: block.gas_used },
            gas_spent_by_tx: gas_spent_by_transactions(receipts),
        })
    }

    // Before Byzantium, receipts contained state root that would mean that expensive
    // operation as hashing that is required for state root got calculated in every
    // transaction This was replaced with is_success flag.
    // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
    if chain_spec.is_byzantium_active_at_block(block.header.number) {
        if let Err(error) =
            verify_receipts(block.header.receipts_root, block.header.logs_bloom, receipts)
        {
            tracing::debug!(%error, ?receipts, "receipts verification failed");
            return Err(error)
        }
    }

    // Validate that the header requests root matches the calculated requests root
    if chain_spec.is_prague_active_at_timestamp(block.timestamp) {
        let Some(header_requests_root) = block.header.requests_root else {
            return Err(ConsensusError::RequestsRootMissing)
        };
        let requests_root = reth_primitives::proofs::calculate_requests_root(requests);
        if requests_root != header_requests_root {
            return Err(ConsensusError::BodyRequestsRootDiff(
                GotExpected::new(requests_root, header_requests_root).into(),
            ))
        }
    }

    Ok(())
}

/// Calculate the receipts root, and compare it against against the expected receipts root and logs
/// bloom.
fn verify_receipts(
    expected_receipts_root: B256,
    expected_logs_bloom: Bloom,
    receipts: &[Receipt],
) -> Result<(), ConsensusError> {
    // Calculate receipts root.
    let receipts_with_bloom = receipts.iter().map(Receipt::with_bloom_ref).collect::<Vec<_>>();
    let receipts_root = reth_primitives::proofs::calculate_receipt_root_ref(&receipts_with_bloom);

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

#[cfg(test)]
mod tests {
    use alloy_primitives::hex;

    use super::*;

    #[test]
    fn test_verify_receipts_success() {
        // Create a vector of 5 default Receipt instances
        let receipts = vec![Receipt::default(); 5];

        // Compare against expected values
        assert!(verify_receipts(
            B256::from(hex!("61353b4fb714dc1fccacbf7eafc4273e62f3d1eed716fe41b2a0cd2e12c63ebc")),
            Bloom::from(hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000")),
            &receipts
        )
        .is_ok());
    }

    #[test]
    fn test_verify_receipts_incorrect_root() {
        // Generate random expected values to produce a failure
        let expected_receipts_root = B256::random();
        let expected_logs_bloom = Bloom::random();

        // Create a vector of 5 random Receipt instances
        let receipts = vec![Receipt::default(); 5];

        assert!(verify_receipts(expected_receipts_root, expected_logs_bloom, &receipts).is_err());
    }

    #[test]
    fn test_compare_receipts_root_and_logs_bloom_success() {
        let calculated_receipts_root = B256::random();
        let calculated_logs_bloom = Bloom::random();

        let expected_receipts_root = calculated_receipts_root;
        let expected_logs_bloom = calculated_logs_bloom;

        assert!(compare_receipts_root_and_logs_bloom(
            calculated_receipts_root,
            calculated_logs_bloom,
            expected_receipts_root,
            expected_logs_bloom
        )
        .is_ok());
    }

    #[test]
    fn test_compare_receipts_root_failure() {
        let calculated_receipts_root = B256::random();
        let calculated_logs_bloom = Bloom::random();

        let expected_receipts_root = B256::random();
        let expected_logs_bloom = calculated_logs_bloom;

        assert_eq!(
            compare_receipts_root_and_logs_bloom(
                calculated_receipts_root,
                calculated_logs_bloom,
                expected_receipts_root,
                expected_logs_bloom
            ),
            Err(ConsensusError::BodyReceiptRootDiff(
                GotExpected { got: calculated_receipts_root, expected: expected_receipts_root }
                    .into()
            ))
        );
    }

    #[test]
    fn test_compare_log_bloom_failure() {
        let calculated_receipts_root = B256::random();
        let calculated_logs_bloom = Bloom::random();

        let expected_receipts_root = calculated_receipts_root;
        let expected_logs_bloom = Bloom::random();

        assert_eq!(
            compare_receipts_root_and_logs_bloom(
                calculated_receipts_root,
                calculated_logs_bloom,
                expected_receipts_root,
                expected_logs_bloom
            ),
            Err(ConsensusError::BodyBloomLogDiff(
                GotExpected { got: calculated_logs_bloom, expected: expected_logs_bloom }.into()
            ))
        );
    }
}
