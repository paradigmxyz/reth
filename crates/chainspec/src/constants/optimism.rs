use alloy_eips::eip1559::BaseFeeParams;
use reth_primitives_traits::constants::{
    BASE_SEPOLIA_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
    OP_MAINNET_EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR_CANYON,
    OP_MAINNET_EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
    OP_MAINNET_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
    OP_SEPOLIA_EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR_CANYON,
    OP_SEPOLIA_EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
    OP_SEPOLIA_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
};

/// Get the base fee parameters for Base Sepolia.
pub(crate) const BASE_SEPOLIA_BASE_FEE_PARAMS: BaseFeeParams = BaseFeeParams {
    max_change_denominator: OP_SEPOLIA_EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
    elasticity_multiplier: BASE_SEPOLIA_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
};

/// Get the base fee parameters for Base Sepolia (post Canyon).
pub(crate) const BASE_SEPOLIA_CANYON_BASE_FEE_PARAMS: BaseFeeParams = BaseFeeParams {
    max_change_denominator: OP_SEPOLIA_EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR_CANYON,
    elasticity_multiplier: BASE_SEPOLIA_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
};

/// Get the base fee parameters for Optimism Sepolia.
pub(crate) const OP_SEPOLIA_BASE_FEE_PARAMS: BaseFeeParams = BaseFeeParams {
    max_change_denominator: OP_SEPOLIA_EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
    elasticity_multiplier: OP_SEPOLIA_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
};

/// Get the base fee parameters for Optimism Sepolia (post Canyon).
pub(crate) const OP_SEPOLIA_CANYON_BASE_FEE_PARAMS: BaseFeeParams = BaseFeeParams {
    max_change_denominator: OP_SEPOLIA_EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR_CANYON,
    elasticity_multiplier: OP_SEPOLIA_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
};

/// Get the base fee parameters for Optimism Mainnet.
pub(crate) const OP_BASE_FEE_PARAMS: BaseFeeParams = BaseFeeParams {
    max_change_denominator: OP_MAINNET_EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
    elasticity_multiplier: OP_MAINNET_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
};

/// Get the base fee parameters for Optimism Mainnet (post Canyon).
pub(crate) const OP_CANYON_BASE_FEE_PARAMS: BaseFeeParams = BaseFeeParams {
    max_change_denominator: OP_MAINNET_EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR_CANYON,
    elasticity_multiplier: OP_MAINNET_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
};
