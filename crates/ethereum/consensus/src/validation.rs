use alloc::vec::Vec;
use alloy_consensus::{proofs::calculate_receipt_root, BlockHeader, TxReceipt};
use alloy_eips::{eip7685::Requests, Encodable2718};
use alloy_primitives::{Bloom, Bytes, B256};
use reth_chainspec::EthereumHardforks;
use reth_consensus::ConsensusError;
use reth_primitives_traits::{
    receipt::gas_spent_by_transactions, Block, GotExpected, Receipt, RecoveredBlock,
};

/// Validate a block with regard to execution results:
///
/// - Compares the receipts root in the block header to the block body
/// - Compares the gas used in the block header to the actual gas usage after execution
///
/// If `receipt_root_bloom` is provided, the pre-computed receipt root and logs bloom are used
/// instead of computing them from the receipts.
///
/// `gas_spent` is the gas_used value from the block execution result. When EIP-7778
/// (Amsterdam) is active, block header gas_used tracks gas before refunds while receipt
/// cumulative_gas_used tracks gas after refunds. In that case, the header must be validated
/// against the execution result's gas_used rather than the receipt value.
pub fn validate_block_post_execution<B, R, ChainSpec>(
    block: &RecoveredBlock<B>,
    chain_spec: &ChainSpec,
    receipts: &[R],
    requests: &Requests,
    receipt_root_bloom: Option<(B256, Bloom)>,
    block_access_list_hash: &Option<B256>,
    gas_spent: Option<u64>,
) -> Result<(), ConsensusError>
where
    B: Block,
    R: Receipt,
    ChainSpec: EthereumHardforks,
{
    // EIP-7778: When Amsterdam is active, block header gas_used tracks gas before refunds,
    // but receipt cumulative_gas_used still tracks gas after refunds. Use the execution
    // result's gas_used which always matches the header semantics.
    let cumulative_gas_used =
        if chain_spec.is_amsterdam_active_at_timestamp(block.header().timestamp()) {
            gas_spent.unwrap_or_default()
        } else {
            receipts.last().map(|receipt| receipt.cumulative_gas_used()).unwrap_or(0)
        };
    if block.header().gas_used() != cumulative_gas_used {
        return Err(ConsensusError::BlockGasUsed {
            gas: GotExpected { got: cumulative_gas_used, expected: block.header().gas_used() },
            gas_spent_by_tx: gas_spent_by_transactions(receipts),
        })
    }

    if chain_spec.is_amsterdam_active_at_timestamp(block.header().number()) {
        let block_bal_hash = block.header().block_access_list_hash().unwrap_or_default();
        if block_access_list_hash.unwrap_or_default() != block_bal_hash {
            return Err(ConsensusError::BlockAccessListHashMismatch(
                (block_access_list_hash.unwrap_or_default(), block_bal_hash).into(),
            ))
        }
    }

    // Before Byzantium, receipts contained state root that would mean that expensive
    // operation as hashing that is required for state root got calculated in every
    // transaction This was replaced with is_success flag.
    // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
    if chain_spec.is_byzantium_active_at_block(block.header().number()) {
        let result = if let Some((receipts_root, logs_bloom)) = receipt_root_bloom {
            compare_receipts_root_and_logs_bloom(
                receipts_root,
                logs_bloom,
                block.header().receipts_root(),
                block.header().logs_bloom(),
            )
        } else {
            verify_receipts(block.header().receipts_root(), block.header().logs_bloom(), receipts)
        };

        if let Err(error) = result {
            let receipts = receipts
                .iter()
                .map(|r| Bytes::from(r.with_bloom_ref().encoded_2718()))
                .collect::<Vec<_>>();
            tracing::debug!(%error, ?receipts, "receipts verification failed");
            return Err(error)
        }
    }

    // Validate that the header requests hash matches the calculated requests hash
    if chain_spec.is_prague_active_at_timestamp(block.header().timestamp()) {
        let Some(header_requests_hash) = block.header().requests_hash() else {
            return Err(ConsensusError::RequestsHashMissing)
        };
        let requests_hash = requests.requests_hash();
        if requests_hash != header_requests_hash {
            return Err(ConsensusError::BodyRequestsHashDiff(
                GotExpected::new(requests_hash, header_requests_hash).into(),
            ))
        }
    }

    Ok(())
}

/// Calculate the receipts root, and compare it against the expected receipts root and logs
/// bloom.
fn verify_receipts<R: Receipt>(
    expected_receipts_root: B256,
    expected_logs_bloom: Bloom,
    receipts: &[R],
) -> Result<(), ConsensusError> {
    // Calculate receipts root.
    let receipts_with_bloom = receipts.iter().map(TxReceipt::with_bloom_ref).collect::<Vec<_>>();
    let receipts_root = calculate_receipt_root(&receipts_with_bloom);

    // Calculate header logs bloom.
    let logs_bloom = receipts_with_bloom.iter().fold(Bloom::ZERO, |bloom, r| bloom | r.bloom_ref());

    compare_receipts_root_and_logs_bloom(
        receipts_root,
        logs_bloom,
        expected_receipts_root,
        expected_logs_bloom,
    )
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
    use super::*;
    use alloy_primitives::{b256, hex};
    use reth_ethereum_primitives::Receipt;

    #[test]
    fn test_verify_receipts_success() {
        // Create a vector of 5 default Receipt instances
        let receipts: Vec<Receipt> = vec![Receipt::default(); 5];

        // Compare against expected values
        assert!(verify_receipts(
            b256!("0x61353b4fb714dc1fccacbf7eafc4273e62f3d1eed716fe41b2a0cd2e12c63ebc"),
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
        let receipts: Vec<Receipt> = vec![Receipt::default(); 5];

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

        assert!(matches!(
            compare_receipts_root_and_logs_bloom(
                calculated_receipts_root,
                calculated_logs_bloom,
                expected_receipts_root,
                expected_logs_bloom
            ).unwrap_err(),
            ConsensusError::BodyReceiptRootDiff(diff)
                if diff.got == calculated_receipts_root && diff.expected == expected_receipts_root
        ));
    }

    #[test]
    fn test_compare_log_bloom_failure() {
        let calculated_receipts_root = B256::random();
        let calculated_logs_bloom = Bloom::random();

        let expected_receipts_root = calculated_receipts_root;
        let expected_logs_bloom = Bloom::random();

        assert!(matches!(
            compare_receipts_root_and_logs_bloom(
                calculated_receipts_root,
                calculated_logs_bloom,
                expected_receipts_root,
                expected_logs_bloom
            ).unwrap_err(),
            ConsensusError::BodyBloomLogDiff(diff)
                if diff.got == calculated_logs_bloom && diff.expected == expected_logs_bloom
        ));
    }
}
