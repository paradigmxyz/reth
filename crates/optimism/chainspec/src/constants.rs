//! OP stack variation of chain spec constants.

use alloy_primitives::{address, b256};
use reth_chainspec::DepositContract;

//------------------------------- BASE MAINNET -------------------------------//

/// Max gas limit on Base: <https://basescan.org/block/17208876>
pub const BASE_MAINNET_MAX_GAS_LIMIT: u64 = 105_000_000;

//------------------------------- BASE SEPOLIA -------------------------------//

/// Max gas limit on Base Sepolia: <https://sepolia.basescan.org/block/12506483>
pub const BASE_SEPOLIA_MAX_GAS_LIMIT: u64 = 45_000_000;

/// Deposit contract address: `0x00000000219ab540356cbb839cbe05303d7705fa`
pub(crate) const MAINNET_DEPOSIT_CONTRACT: DepositContract = DepositContract::new(
    address!("00000000219ab540356cbb839cbe05303d7705fa"),
    11052984,
    b256!("649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"),
);
