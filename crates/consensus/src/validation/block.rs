//! Collection of methods for block validation.
use reth_interfaces::{consensus::Error, Result as RethResult};
use reth_primitives::{
    constants::{EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR, EIP1559_ELASTICITY_MULTIPLIER},
    proofs, Header, SealedBlock,
};
use reth_provider::HeaderProvider;

/// Validate a block without regard for state:
///
/// - Compares the ommer hash in the block header to the block body
/// - Compares the transactions root in the block header to the block body
/// - Pre-execution transaction validation
/// - (Optionally) Compares the receipts root in the block header to the block body
pub fn validate_block_standalone(block: &SealedBlock) -> Result<(), Error> {
    // Check ommers hash
    // TODO(onbjerg): This should probably be accessible directly on [Block]
    let ommers_hash = proofs::calculate_ommers_root(block.ommers.iter().map(|h| h.as_ref()));
    if block.header.ommers_hash != ommers_hash {
        return Err(Error::BodyOmmersHashDiff {
            got: ommers_hash,
            expected: block.header.ommers_hash,
        })
    }

    // Check transaction root
    // TODO(onbjerg): This should probably be accessible directly on [Block]
    let transaction_root = proofs::calculate_transaction_root(block.body.iter());
    if block.header.transactions_root != transaction_root {
        return Err(Error::BodyTransactionRootDiff {
            got: transaction_root,
            expected: block.header.transactions_root,
        })
    }

    Ok(())
}

/// Calculate base fee for next block. EIP-1559 spec
pub fn calculate_next_block_base_fee(gas_used: u64, gas_limit: u64, base_fee: u64) -> u64 {
    let gas_target = gas_limit / EIP1559_ELASTICITY_MULTIPLIER;

    if gas_used == gas_target {
        return base_fee
    }
    if gas_used > gas_target {
        let gas_used_delta = gas_used - gas_target;
        let base_fee_delta = std::cmp::max(
            1,
            base_fee as u128 * gas_used_delta as u128 /
                gas_target as u128 /
                EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR as u128,
        );
        base_fee + (base_fee_delta as u64)
    } else {
        let gas_used_delta = gas_target - gas_used;
        let base_fee_per_gas_delta = base_fee as u128 * gas_used_delta as u128 /
            gas_target as u128 /
            EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR as u128;

        base_fee.saturating_sub(base_fee_per_gas_delta as u64)
    }
}

/// Validate block in regards to chain (parent)
///
/// Checks:
///  If we already know the block.
///  If parent is known
///
/// Returns parent block header  
pub fn validate_block_regarding_chain<P: HeaderProvider>(
    block: &SealedBlock,
    provider: &P,
) -> RethResult<Header> {
    let hash = block.header.hash();

    // Check if block is known.
    if provider.is_known(&hash)? {
        return Err(Error::BlockKnown { hash, number: block.header.number }.into())
    }

    // Check if parent is known.
    let parent = provider
        .header(&block.parent_hash)?
        .ok_or(Error::ParentUnknown { hash: block.parent_hash })?;

    // Return parent header.
    Ok(parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_base_fee_success() {
        let base_fee = [
            1000000000, 1000000000, 1000000000, 1072671875, 1059263476, 1049238967, 1049238967, 0,
            1, 2,
        ];
        let gas_used = [
            10000000, 10000000, 10000000, 9000000, 10001000, 0, 10000000, 10000000, 10000000,
            10000000,
        ];
        let gas_limit = [
            10000000, 12000000, 14000000, 10000000, 14000000, 2000000, 18000000, 18000000,
            18000000, 18000000,
        ];
        let next_base_fee = [
            1125000000, 1083333333, 1053571428, 1179939062, 1116028649, 918084097, 1063811730, 1,
            2, 3,
        ];

        for i in 0..base_fee.len() {
            assert_eq!(
                next_base_fee[i],
                calculate_next_block_base_fee(gas_used[i], gas_limit[i], base_fee[i])
            );
        }
    }
}
