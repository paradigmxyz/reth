//! Ethereum protocol-related constants

use crate::H256;
use hex_literal::hex;

/// Initial base fee as defined in [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559)
pub const EIP1559_INITIAL_BASE_FEE: u64 = 1_000_000_000;

/// Base fee max change denominator as defined in [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559)
pub const EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR: u64 = 8;

/// Elasticity multiplier as defined in [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559)
pub const EIP1559_ELASTICITY_MULTIPLIER: u64 = 2;

/// Multiplier for converting gwei to wei.
pub const GWEI_TO_WEI: u64 = 1_000_000_000;

/// The Ethereum mainnet genesis hash.
pub const MAINNET_GENESIS: H256 =
    H256(hex!("d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"));

/// Goerli genesis hash.
pub const GOERLI_GENESIS: H256 =
    H256(hex!("bf7e331f7f7c1dd2e05159666b3bf8bc7a8a3a9eb1d518969eab529dd9b88c1a"));

/// Sepolia genesis hash.
pub const SEPOLIA_GENESIS: H256 =
    H256(hex!("25a5cc106eea7138acab33231d7160d69cb777ee0c2c553fcddf5138993e6dd9"));

/// Keccak256 over empty array.
pub const KECCAK_EMPTY: H256 =
    H256(hex!("c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"));

/// Ommer root of empty list.
pub const EMPTY_OMMER_ROOT: H256 =
    H256(hex!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"));
