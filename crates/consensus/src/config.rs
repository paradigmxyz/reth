//! Reth block execution/validation configuration and constants

/// Initial base fee as defined in: https://eips.ethereum.org/EIPS/eip-1559
pub const EIP1559_INITIAL_BASE_FEE: u64 = 1_000_000_000;
/// Base fee max change denominator as defined in: https://eips.ethereum.org/EIPS/eip-1559
pub const EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR: u64 = 8;
/// Elasticity multiplier as defined in: https://eips.ethereum.org/EIPS/eip-1559
pub const EIP1559_ELASTICITY_MULTIPLIER: u64 = 2;
