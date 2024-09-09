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

/// Maximum extra data size in a block after genesis
pub const MAXIMUM_EXTRA_DATA_SIZE: usize = 32;

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

/// Multiplier for converting gwei to wei.
pub const GWEI_TO_WEI: u64 = 1_000_000_000;

/// Multiplier for converting finney (milliether) to wei.
pub const FINNEY_TO_WEI: u128 = (GWEI_TO_WEI as u128) * 1_000_000;

/// Multiplier for converting ether to wei.
pub const ETH_TO_WEI: u128 = FINNEY_TO_WEI * 1000;

/// The Ethereum mainnet genesis hash:
/// `0x0d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3`
pub const MAINNET_GENESIS_HASH: B256 =
    b256!("d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3");

/// Sepolia genesis hash: `0x25a5cc106eea7138acab33231d7160d69cb777ee0c2c553fcddf5138993e6dd9`
pub const SEPOLIA_GENESIS_HASH: B256 =
    b256!("25a5cc106eea7138acab33231d7160d69cb777ee0c2c553fcddf5138993e6dd9");

/// Holesky genesis hash: `0xb5f7f912443c940f21fd611f12828d75b534364ed9e95ca4e307729a4661bde4`
pub const HOLESKY_GENESIS_HASH: B256 =
    b256!("b5f7f912443c940f21fd611f12828d75b534364ed9e95ca4e307729a4661bde4");

/// Testnet genesis hash: `0x2f980576711e3617a5e4d83dd539548ec0f7792007d505a3d2e9674833af2d7c`
pub const DEV_GENESIS_HASH: B256 =
    b256!("2f980576711e3617a5e4d83dd539548ec0f7792007d505a3d2e9674833af2d7c");

/// Keccak256 over empty array: `0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470`
pub const KECCAK_EMPTY: B256 =
    b256!("c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470");

/// Ommer root of empty list: `0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347`
pub const EMPTY_OMMER_ROOT_HASH: B256 =
    b256!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347");

/// Root hash of an empty trie: `0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421`
pub const EMPTY_ROOT_HASH: B256 =
    b256!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421");

/// From address from Optimism system txs: `0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001`
pub const OP_SYSTEM_TX_FROM_ADDR: Address = address!("deaddeaddeaddeaddeaddeaddeaddeaddead0001");

/// To address from Optimism system txs: `0x4200000000000000000000000000000000000015`
pub const OP_SYSTEM_TX_TO_ADDR: Address = address!("4200000000000000000000000000000000000015");

/// Transactions root of empty receipts set.
pub const EMPTY_RECEIPTS: B256 = EMPTY_ROOT_HASH;

/// Transactions root of empty transactions set.
pub const EMPTY_TRANSACTIONS: B256 = EMPTY_ROOT_HASH;

/// Withdrawals root of empty withdrawals set.
pub const EMPTY_WITHDRAWALS: B256 = EMPTY_ROOT_HASH;

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
