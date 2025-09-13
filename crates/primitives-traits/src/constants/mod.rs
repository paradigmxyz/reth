//! Ethereum protocol-related constants

/// Gas units, for example [`GIGAGAS`].
pub mod gas_units;
pub use gas_units::{GIGAGAS, KILOGAS, MEGAGAS};

/// The client version: `reth/v{major}.{minor}.{patch}`
pub const RETH_CLIENT_VERSION: &str = concat!("reth/v", env!("CARGO_PKG_VERSION"));

/// Minimum gas limit allowed for transactions.
pub const MINIMUM_GAS_LIMIT: u64 = 5000;

/// Maximum gas limit allowed for block.
/// In hex this number is `0x7fffffffffffffff`
pub const MAXIMUM_GAS_LIMIT_BLOCK: u64 = 2u64.pow(63) - 1;

/// The bound divisor of the gas limit, used in update calculations.
pub const GAS_LIMIT_BOUND_DIVISOR: u64 = 1024;

/// Maximum transaction gas limit as defined by [EIP-7825](https://eips.ethereum.org/EIPS/eip-7825) activated in `Osaka` hardfork.
pub const MAX_TX_GAS_LIMIT_OSAKA: u64 = 30_000_000;

/// The number of blocks to unwind during a reorg that already became a part of canonical chain.
///
/// In reality, the node can end up in this particular situation very rarely. It would happen only
/// if the node process is abruptly terminated during ongoing reorg and doesn't boot back up for
/// long period of time.
///
/// Unwind depth of `3` blocks significantly reduces the chance that the reorged block is kept in
/// the database.
pub const BEACON_CONSENSUS_REORG_UNWIND_DEPTH: u64 = 3;
