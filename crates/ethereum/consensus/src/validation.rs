use alloc::vec::Vec;
use alloy_consensus::{proofs::calculate_receipt_root, BlockHeader, TxReceipt};
use alloy_eips::{
    eip7928::{compute_block_access_list_hash, BlockAccessList},
    Encodable2718,
};
use alloy_primitives::{Bloom, Bytes, B256};
use reth_chainspec::EthereumHardforks;
use reth_consensus::ConsensusError;
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{
    receipt::gas_spent_by_transactions, Block, GotExpected, Receipt, RecoveredBlock,
};

struct BlockAccessListLogCounts {
    accounts: usize,
    storage_changes: usize,
    storage_reads: usize,
    balance_changes: usize,
    nonce_changes: usize,
    code_changes: usize,
}

impl BlockAccessListLogCounts {
    fn new(block_access_list: &BlockAccessList) -> Self {
        let mut counts = Self {
            accounts: block_access_list.len(),
            storage_changes: 0,
            storage_reads: 0,
            balance_changes: 0,
            nonce_changes: 0,
            code_changes: 0,
        };

        for account in block_access_list {
            counts.storage_changes += account.storage_changes.len();
            counts.storage_reads += account.storage_reads.len();
            counts.balance_changes += account.balance_changes.len();
            counts.nonce_changes += account.nonce_changes.len();
            counts.code_changes += account.code_changes.len();
        }

        counts
    }
}

impl core::fmt::Debug for BlockAccessListLogCounts {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BlockAccessListLogCounts")
            .field("accounts", &self.accounts)
            .field("storage_changes", &self.storage_changes)
            .field("storage_reads", &self.storage_reads)
            .field("balance_changes", &self.balance_changes)
            .field("nonce_changes", &self.nonce_changes)
            .field("code_changes", &self.code_changes)
            .finish()
    }
}

/// Validate a block with regard to execution results:
///
/// - Compares the receipts root in the block header to the block body
/// - Compares the gas used in the block header to the actual gas usage after execution
///
/// If `receipt_root_bloom` is provided, the pre-computed receipt root and logs bloom are used
/// instead of computing them from the receipts.
pub fn validate_block_post_execution<B, R, ChainSpec>(
    block: &RecoveredBlock<B>,
    chain_spec: &ChainSpec,
    result: &BlockExecutionResult<R>,
    receipt_root_bloom: Option<(B256, Bloom)>,
    block_access_list: Option<BlockAccessList>,
) -> Result<(), ConsensusError>
where
    B: Block,
    R: Receipt,
    ChainSpec: EthereumHardforks,
{
    // Check if gas used matches the value set in header.
    if block.header().gas_used() != result.gas_used {
        return Err(ConsensusError::BlockGasUsed {
            gas: GotExpected { got: result.gas_used, expected: block.header().gas_used() },
            gas_spent_by_tx: gas_spent_by_transactions(&result.receipts),
        })
    }

    // Before Byzantium, receipts contained state root that would mean that expensive
    // operation as hashing that is required for state root got calculated in every
    // transaction This was replaced with is_success flag.
    // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
    if chain_spec.is_byzantium_active_at_block(block.header().number()) {
        let res = if let Some((receipts_root, logs_bloom)) = receipt_root_bloom {
            compare_receipts_root_and_logs_bloom(
                receipts_root,
                logs_bloom,
                block.header().receipts_root(),
                block.header().logs_bloom(),
            )
        } else {
            verify_receipts(
                block.header().receipts_root(),
                block.header().logs_bloom(),
                &result.receipts,
            )
        };

        if let Err(error) = res {
            let receipts = result
                .receipts
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
        let requests_hash = result.requests.requests_hash();
        if requests_hash != header_requests_hash {
            return Err(ConsensusError::BodyRequestsHashDiff(
                GotExpected::new(requests_hash, header_requests_hash).into(),
            ))
        }
    }

    // Validate that the block access list hash matches the calculated block access list hash
    if chain_spec.is_amsterdam_active_at_timestamp(block.header().timestamp()) &&
        block_access_list.is_some()
    {
        let block_bal_hash = block.header().block_access_list_hash().unwrap_or_default();
        let default_bal = BlockAccessList::default();
        let block_access_list_hash =
            compute_block_access_list_hash(block_access_list.as_ref().unwrap_or(&default_bal));
        if block_access_list_hash != block_bal_hash {
            let generated_bal = block_access_list.as_ref().unwrap_or(&default_bal);
            let generated_bal_first_accounts =
                generated_bal.iter().take(16).map(|account| account.address).collect::<Vec<_>>();
            tracing::info!(
                target: "consensus::validation",
                block_hash = %block.hash(),
                block_number = block.header().number(),
                generated_bal_hash = %block_access_list_hash,
                header_bal_hash = %block_bal_hash,
                generated_bal_counts = ?BlockAccessListLogCounts::new(generated_bal),
                ?generated_bal_first_accounts,
                generated_bal = ?generated_bal,
                "Mismatched block access list hash after execution"
            );
            return Err(ConsensusError::BlockAccessListHashMismatch(
                (block_access_list_hash, block_bal_hash).into(),
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
