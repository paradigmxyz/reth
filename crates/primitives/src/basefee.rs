//! Helpers for working with EIP-1559 base fee

// re-export
#[doc(inline)]
pub use alloy_eips::eip1559::calc_next_block_base_fee;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "optimism")]
    use crate::chain::{OP_BASE_FEE_PARAMS, OP_SEPOLIA_BASE_FEE_PARAMS};

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
                calc_next_block_base_fee(
                    gas_used[i] as u128,
                    gas_limit[i] as u128,
                    base_fee[i] as u128,
                    crate::BaseFeeParams::ethereum(),
                ) as u64
            );
        }
    }

    #[cfg(feature = "optimism")]
    #[test]
    fn calculate_optimism_base_fee_success() {
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
            1100000048, 1080000000, 1065714297, 1167067046, 1128881311, 1028254188, 1098203452, 1,
            2, 3,
        ];

        for i in 0..base_fee.len() {
            assert_eq!(
                next_base_fee[i],
                calc_next_block_base_fee(
                    gas_used[i] as u128,
                    gas_limit[i] as u128,
                    base_fee[i] as u128,
                    OP_BASE_FEE_PARAMS,
                ) as u64
            );
        }
    }

    #[cfg(feature = "optimism")]
    #[test]
    fn calculate_optimism_sepolia_base_fee_success() {
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
            1180000000, 1146666666, 1122857142, 1244299375, 1189416692, 1028254188, 1144836295, 1,
            2, 3,
        ];

        for i in 0..base_fee.len() {
            assert_eq!(
                next_base_fee[i],
                calc_next_block_base_fee(
                    gas_used[i] as u128,
                    gas_limit[i] as u128,
                    base_fee[i] as u128,
                    OP_SEPOLIA_BASE_FEE_PARAMS,
                ) as u64
            );
        }
    }
}
