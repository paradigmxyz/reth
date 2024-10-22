//! Ethereum protocol-related constants

use alloy_primitives::{address, b256, Address, B256, U256};
use core::time::Duration;

/// Gas units, for example [`GIGAGAS`].
pub mod gas_units;
pub use gas_units::{GIGAGAS, KILOGAS, MEGAGAS};

/// The client version: `reth/v{major}.{minor}.{patch}`
pub const RETH_CLIENT_VERSION: &str = concat!("reth/v", env!("CARGO_PKG_VERSION"));

/// The first four bytes of the call data for a function call specifies the function to be called.
pub const SELECTOR_LEN: usize = 4;

/// An EPOCH is a series of 32 slots.
pub const EPOCH_SLOTS: u64 = 32;

/// The duration of a slot in seconds.
///
/// This is the time period of 12 seconds in which a randomly chosen validator has time to propose a
/// block.
pub const SLOT_DURATION: Duration = Duration::from_secs(12);

/// An EPOCH is a series of 32 slots (~6.4min).
pub const EPOCH_DURATION: Duration = Duration::from_secs(12 * EPOCH_SLOTS);

/// The default block nonce in the beacon consensus
pub const BEACON_NONCE: u64 = 0u64;

/// The default Ethereum block gas limit.
pub const ETHEREUM_BLOCK_GAS_LIMIT: u64 = 30_000_000;

/// The minimum tx fee below which the txpool will reject the transaction.
///
/// Configured to `7` WEI which is the lowest possible value of base fee under mainnet EIP-1559
/// parameters. `BASE_FEE_MAX_CHANGE_DENOMINATOR` <https://eips.ethereum.org/EIPS/eip-1559>
/// is `8`, or 12.5%. Once the base fee has dropped to `7` WEI it cannot decrease further because
/// 12.5% of 7 is less than 1.
///
/// Note that min base fee under different 1559 parameterizations may differ, but there's no
/// significant harm in leaving this setting as is.
pub const MIN_PROTOCOL_BASE_FEE: u64 = 7;

/// Same as [`MIN_PROTOCOL_BASE_FEE`] but as a U256.
pub const MIN_PROTOCOL_BASE_FEE_U256: U256 = U256::from_limbs([7u64, 0, 0, 0]);

/// Initial base fee as defined in [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559)
pub const EIP1559_INITIAL_BASE_FEE: u64 = 1_000_000_000;

/// Base fee max change denominator as defined in [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559)
pub const EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR: u64 = 8;

/// Elasticity multiplier as defined in [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559)
pub const EIP1559_DEFAULT_ELASTICITY_MULTIPLIER: u64 = 2;

/// Minimum gas limit allowed for transactions.
pub const MINIMUM_GAS_LIMIT: u64 = 5000;

/// Base fee max change denominator for Optimism Mainnet as defined in the Optimism
/// [transaction costs](https://community.optimism.io/docs/developers/build/differences/#transaction-costs) doc.
pub const OP_MAINNET_EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR: u128 = 50;

/// Base fee max change denominator for Optimism Mainnet as defined in the Optimism Canyon
/// hardfork.
pub const OP_MAINNET_EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR_CANYON: u128 = 250;

/// Base fee max change denominator for Optimism Mainnet as defined in the Optimism
/// [transaction costs](https://community.optimism.io/docs/developers/build/differences/#transaction-costs) doc.
pub const OP_MAINNET_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER: u128 = 6;

/// Base fee max change denominator for Optimism Sepolia as defined in the Optimism
/// [transaction costs](https://community.optimism.io/docs/developers/build/differences/#transaction-costs) doc.
pub const OP_SEPOLIA_EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR: u128 = 50;

/// Base fee max change denominator for Optimism Sepolia as defined in the Optimism Canyon
/// hardfork.
pub const OP_SEPOLIA_EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR_CANYON: u128 = 250;

/// Base fee max change denominator for Optimism Sepolia as defined in the Optimism
/// [transaction costs](https://community.optimism.io/docs/developers/build/differences/#transaction-costs) doc.
pub const OP_SEPOLIA_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER: u128 = 6;

/// Base fee max change denominator for Base Sepolia as defined in the Optimism
/// [transaction costs](https://community.optimism.io/docs/developers/build/differences/#transaction-costs) doc.
pub const BASE_SEPOLIA_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER: u128 = 10;

/// Sepolia genesis hash: `0x25a5cc106eea7138acab33231d7160d69cb777ee0c2c553fcddf5138993e6dd9`
pub const SEPOLIA_GENESIS_HASH: B256 =
    b256!("25a5cc106eea7138acab33231d7160d69cb777ee0c2c553fcddf5138993e6dd9");

/// Holesky genesis hash: `0xb5f7f912443c940f21fd611f12828d75b534364ed9e95ca4e307729a4661bde4`
pub const HOLESKY_GENESIS_HASH: B256 =
    b256!("b5f7f912443c940f21fd611f12828d75b534364ed9e95ca4e307729a4661bde4");

/// From address from Optimism system txs: `0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001`
pub const OP_SYSTEM_TX_FROM_ADDR: Address = address!("deaddeaddeaddeaddeaddeaddeaddeaddead0001");

/// To address from Optimism system txs: `0x4200000000000000000000000000000000000015`
pub const OP_SYSTEM_TX_TO_ADDR: Address = address!("4200000000000000000000000000000000000015");

/// The number of blocks to unwind during a reorg that already became a part of canonical chain.
///
/// In reality, the node can end up in this particular situation very rarely. It would happen only
/// if the node process is abruptly terminated during ongoing reorg and doesn't boot back up for
/// long period of time.
///
/// Unwind depth of `3` blocks significantly reduces the chance that the reorged block is kept in
/// the database.
pub const BEACON_CONSENSUS_REORG_UNWIND_DEPTH: u64 = 3;

/// Max seconds from current time allowed for blocks, before they're considered future blocks.
///
/// This is only used when checking whether or not the timestamp for pre-merge blocks is in the
/// future.
///
/// See:
/// <https://github.com/ethereum/go-ethereum/blob/a196f3e8a22b6ad22ced5c2e3baf32bc3ebd4ec9/consensus/ethash/consensus.go#L227-L229>
pub const ALLOWED_FUTURE_BLOCK_TIME_SECONDS: u64 = 15;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_protocol_sanity() {
        assert_eq!(MIN_PROTOCOL_BASE_FEE_U256.to::<u64>(), MIN_PROTOCOL_BASE_FEE);
    }
}
