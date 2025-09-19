use alloc::vec::Vec;
use alloy_consensus::{proofs::calculate_receipt_root, BlockHeader, TxReceipt};
use alloy_eips::{eip7685::Requests, Encodable2718};
use alloy_primitives::{Bloom, Bytes, B256};
use reth_chainspec::{DepositContract, EthereumHardforks}; 
use reth_consensus::ConsensusError;
use reth_primitives_traits::{
    receipt::gas_spent_by_transactions, Block, GotExpected, Receipt, RecoveredBlock,
};

/// Temp for proof of concept
pub trait DepositContractProvider {
    /// Womp
    fn get_deposit_contract(&self) -> Option<DepositContract>;
}

/// Temp for proof of concept
impl DepositContractProvider for reth_chainspec::ChainSpec {
    fn get_deposit_contract(&self) -> Option<DepositContract> {
        self.deposit_contract
    }
}

/// Validate a block with regard to execution results:
///
/// - Compares the receipts root in the block header to the block body
/// - Compares the gas used in the block header to the actual gas usage after execution
pub fn validate_block_post_execution<B, R, CS>(
    block: &RecoveredBlock<B>,
    chain_spec: &CS,
    receipts: &[R],
    requests: &Requests,
) -> Result<(), ConsensusError>
where
    B: Block,
    R: Receipt,
    CS: EthereumHardforks + DepositContractProvider,
{
    // Check if gas used matches the value set in header.
    let cumulative_gas_used =
        receipts.last().map(|receipt| receipt.cumulative_gas_used()).unwrap_or(0);
    if block.header().gas_used() != cumulative_gas_used {
        return Err(ConsensusError::BlockGasUsed {
            gas: GotExpected { got: cumulative_gas_used, expected: block.header().gas_used() },
            gas_spent_by_tx: gas_spent_by_transactions(receipts),
        })
    }

    // Before Byzantium, receipts contained state root that would mean that expensive
    // operation as hashing that is required for state root got calculated in every
    // transaction This was replaced with is_success flag.
    // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
    if chain_spec.is_byzantium_active_at_block(block.header().number()) {
        if let Err(error) =
            verify_receipts(block.header().receipts_root(), block.header().logs_bloom(), receipts)
        {
            let receipts = receipts
                .iter()
                .map(|r| Bytes::from(r.with_bloom_ref().encoded_2718()))
                .collect::<Vec<_>>();
            tracing::debug!(%error, ?receipts, "receipts verification failed");
            return Err(error)
        }
    }

    // Validate deposit event data layout and requests hash
    if chain_spec.is_prague_active_at_timestamp(block.header().timestamp()) {
        // Validate deposit event data layout
        if let Some(deposit_contract) = chain_spec.get_deposit_contract() {
            for receipt in receipts {
                for log in receipt.logs() {
                    if log.address == deposit_contract.address 
                        && log.topics().first() == Some(&deposit_contract.topic) 
                    {
                        if !verify_deposit_event_data(&log.data.data) {
                            return Err(ConsensusError::InvalidDepositEventLayout);
                        }
                    }
                }
            }
        }
    
        // Validate requests hash
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

/// Validates deposit event data according to EIP-6110 specification
fn verify_deposit_event_data(deposit_event_data: &[u8]) -> bool {
    // Check if the event data length is exactly 576 bytes
    if deposit_event_data.len() != 576 {
        return false;
    }

    // Extract offsets from the first 160 bytes
    let pubkey_offset = u32::from_be_bytes([
        deposit_event_data[28], deposit_event_data[29], 
        deposit_event_data[30], deposit_event_data[31]
    ]) as usize;
    
    let withdrawal_credentials_offset = u32::from_be_bytes([
        deposit_event_data[60], deposit_event_data[61], 
        deposit_event_data[62], deposit_event_data[63]
    ]) as usize;
    
    let amount_offset = u32::from_be_bytes([
        deposit_event_data[92], deposit_event_data[93],
        deposit_event_data[94], deposit_event_data[95]
    ]) as usize;
    
    let signature_offset = u32::from_be_bytes([
        deposit_event_data[124], deposit_event_data[125],
        deposit_event_data[126], deposit_event_data[127]
    ]) as usize;
    
    let index_offset = u32::from_be_bytes([
        deposit_event_data[156], deposit_event_data[157],
        deposit_event_data[158], deposit_event_data[159]
    ]) as usize;

    // Validate the expected offset values
    if pubkey_offset != 160
        || withdrawal_credentials_offset != 256
        || amount_offset != 320
        || signature_offset != 384
        || index_offset != 512
    {
        return false;
    }

    // Check bounds for all offsets
    if pubkey_offset + 32 > deposit_event_data.len()
        || withdrawal_credentials_offset + 32 > deposit_event_data.len()
        || amount_offset + 32 > deposit_event_data.len()
        || signature_offset + 32 > deposit_event_data.len()
        || index_offset + 32 > deposit_event_data.len()
    {
        return false;
    }

    // Extract sizes from the offset locations
    let pubkey_size = u32::from_be_bytes([
        deposit_event_data[pubkey_offset + 28],
        deposit_event_data[pubkey_offset + 29],
        deposit_event_data[pubkey_offset + 30],
        deposit_event_data[pubkey_offset + 31]
    ]);
    
    let withdrawal_credentials_size = u32::from_be_bytes([
        deposit_event_data[withdrawal_credentials_offset + 28],
        deposit_event_data[withdrawal_credentials_offset + 29],
        deposit_event_data[withdrawal_credentials_offset + 30],
        deposit_event_data[withdrawal_credentials_offset + 31]
    ]);
    
    let amount_size = u32::from_be_bytes([
        deposit_event_data[amount_offset + 28],
        deposit_event_data[amount_offset + 29],
        deposit_event_data[amount_offset + 30],
        deposit_event_data[amount_offset + 31]
    ]);
    
    let signature_size = u32::from_be_bytes([
        deposit_event_data[signature_offset + 28],
        deposit_event_data[signature_offset + 29],
        deposit_event_data[signature_offset + 30],
        deposit_event_data[signature_offset + 31]
    ]);
    
    let index_size = u32::from_be_bytes([
        deposit_event_data[index_offset + 28],
        deposit_event_data[index_offset + 29],
        deposit_event_data[index_offset + 30],
        deposit_event_data[index_offset + 31]
    ]);

    // Validate the expected sizes
    pubkey_size == 48
        && withdrawal_credentials_size == 32
        && amount_size == 8
        && signature_size == 96
        && index_size == 8
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
