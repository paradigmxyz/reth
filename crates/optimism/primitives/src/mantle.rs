//! Mantle Sepolia related data.

use alloy_consensus::{Header, EMPTY_OMMER_ROOT_HASH, EMPTY_ROOT_HASH};
use alloy_primitives::{address, b256, bloom, bytes, B256, B64, U256};

/// The Mantle Sepolia chain ID.
pub const MANTLE_SEPOLIA_CHAIN_ID: u64 = 5003;

/// Mantle Sepolia genesis header hash.
pub const MANTLE_SEPOLIA_HEADER_HASH: B256 =
    b256!("0x4a90feadca38863170c904666d9062f6bc50d2f790962adf819260cba5dba9f5");

/// Mantle Sepolia genesis header.(temporary)
/// 22,710,000
/// <https://explorer.sepolia.mantle.xyz/block/0x4a90feadca38863170c904666d9062f6bc50d2f790962adf819260cba5dba9f5>
pub const MANTLE_SEPOLIA_HEADER: Header = Header {
    difficulty: U256::ZERO,
    extra_data: bytes!("0x"),
    gas_limit: 200000000000,
    gas_used: 2222742961,
    logs_bloom: bloom!(
        "00000080000001000000000400000000000000000000000000000000800000200000000040004000000000000000000800000000004000000000000000000000000010000000000000000008004000000000000020000000000000000000001000000000000004400000000000000000000000000000000000000010000040000000000000020000000000000000000000000004000001000000000040082000800000000000020000000000000000000000000000000000000000000000000000100002000000021002004000000000000010000000008000000100000000000000000008000000000000004000000000000000000000000000000000200000"
    ),
    nonce: B64::ZERO,
    number: 22710000,
    parent_hash: b256!("0x80b74c6a50e10cacf86487c411e6a989eb6b3cb857a469ed9803ea17858e1dfa"),
    receipts_root: b256!("0x48e6d960b2046a972457e24cf71892c6e84c2496c299b01a6d6dc5cd9b9c8ed7"),
    state_root: b256!("0xa1021f23d9e219fba5765a6b23f349532455c35e4a999e60e68c7662fa9a7538"),
    timestamp: 1746964673,
    transactions_root: b256!("0x54b898419adf350c9e92a325a8e3d2a5107e40b12c58ec45a05833cb1d6248ea"),
    ommers_hash: b256!("0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"),
    beneficiary: address!("0x4200000000000000000000000000000000000011"),
    withdrawals_root: None,
    mix_hash: b256!("0x46e2deaaa2f7bfc1b5336754f024c34c85eb0d1725b324e43a8d0b0c675defeb"),
    base_fee_per_gas: Some(0x1312d00),
    blob_gas_used: None,
    excess_blob_gas: None,
    parent_beacon_block_root: None,
    requests_hash: None,
};

/// Mantle Sepolia total difficulty on Sepolia.
pub const MANTLE_SEPOLIA_HEADER_TTD: U256 = U256::ZERO;


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mantle_sepolia_header() {
        assert_eq!(MANTLE_SEPOLIA_HEADER.hash_slow(), MANTLE_SEPOLIA_HEADER_HASH);
    }
}
