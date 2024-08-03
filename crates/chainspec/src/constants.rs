use crate::spec::DepositContract;
use alloy_primitives::{address, b256};

/// Deposit contract address: `0x00000000219ab540356cbb839cbe05303d7705fa`
pub(crate) const MAINNET_DEPOSIT_CONTRACT: DepositContract = DepositContract::new(
    address!("00000000219ab540356cbb839cbe05303d7705fa"),
    11052984,
    b256!("649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"),
);

/// Max gas limit on Base Sepolia: <https://sepolia.basescan.org/block/12506483>
#[cfg(feature = "optimism")]
pub(crate) const BASE_SEPOLIA_MAX_GAS_LIMIT: u64 = 45_000_000;

/// Max gas limit on Base: <https://basescan.org/block/17208876>
#[cfg(feature = "optimism")]
pub(crate) const BASE_MAINNET_MAX_GAS_LIMIT: u64 = 105_000_000;
