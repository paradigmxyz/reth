use crate::constants::{
    EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR, EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
};
use serde::{Deserialize, Serialize};

/// BaseFeeParams contains the config parameters that control block base fee computation
#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq)]
pub struct BaseFeeParams {
    /// The base_fee_max_change_denominator from EIP-1559
    pub max_change_denominator: u64,
    /// The elasticity multiplier from EIP-1559
    pub elasticity_multiplier: u64,
}

impl BaseFeeParams {
    /// Get the base fee parameters for Ethereum mainnet
    pub const fn ethereum() -> BaseFeeParams {
        BaseFeeParams {
            max_change_denominator: EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
            elasticity_multiplier: EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
        }
    }

    /// Get the base fee parameters for optimism goerli
    #[cfg(feature = "optimism")]
    pub const fn optimism_goerli() -> BaseFeeParams {
        BaseFeeParams {
            max_change_denominator:
                crate::constants::OP_GOERLI_EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
            elasticity_multiplier:
                crate::constants::OP_GOERLI_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
        }
    }

    /// Get the base fee parameters for optimism mainnet
    #[cfg(feature = "optimism")]
    pub const fn optimism() -> BaseFeeParams {
        BaseFeeParams {
            max_change_denominator:
                crate::constants::OP_MAINNET_EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
            elasticity_multiplier:
                crate::constants::OP_MAINNET_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
        }
    }
}
