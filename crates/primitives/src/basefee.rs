//! Helpers for working with EIP-1559 base fee

/// Calculate base fee for next block. [EIP-1559](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-1559.md) spec
pub fn calculate_next_block_base_fee(
    gas_used: u64,
    gas_limit: u64,
    base_fee: u64,
    base_fee_params: crate::BaseFeeParams,
) -> u64 {
    let gas_target = gas_limit / base_fee_params.elasticity_multiplier;
    if gas_used == gas_target {
        return base_fee
    }
    if gas_used > gas_target {
        let gas_used_delta = gas_used - gas_target;
        let base_fee_delta = std::cmp::max(
            1,
            base_fee as u128 * gas_used_delta as u128 /
                gas_target as u128 /
                base_fee_params.max_change_denominator as u128,
        );
        base_fee + (base_fee_delta as u64)
    } else {
        let gas_used_delta = gas_target - gas_used;
        let base_fee_per_gas_delta = base_fee as u128 * gas_used_delta as u128 /
            gas_target as u128 /
            base_fee_params.max_change_denominator as u128;

        base_fee.saturating_sub(base_fee_per_gas_delta as u64)
    }
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
                calculate_next_block_base_fee(
                    gas_used[i],
                    gas_limit[i],
                    base_fee[i],
                    crate::BaseFeeParams::ethereum(),
                )
            );
        }
    }
}
