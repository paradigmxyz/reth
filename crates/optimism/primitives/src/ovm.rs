//! OP mainnet OVM related data.

use alloy_primitives::{b256, bloom, bytes, Address, B256, U256};
use reth_primitives::Header;
use reth_primitives_traits::constants::EMPTY_OMMER_ROOT_HASH;

/// Transaction 0x9ed8f713b2cc6439657db52dcd2fdb9cc944915428f3c6e2a7703e242b259cb9 in block 985,
/// replayed in blocks:
///
/// 19 022
/// 45 036
pub const TX_BLOCK_985: [u64; 2] = [19_022, 45_036];

/// Transaction 0xc033250c5a45f9d104fc28640071a776d146d48403cf5e95ed0015c712e26cb6 in block
/// 123 322, replayed in block:
///
/// 123 542
pub const TX_BLOCK_123_322: u64 = 123_542;

/// Transaction 0x86f8c77cfa2b439e9b4e92a10f6c17b99fce1220edf4001e4158b57f41c576e5 in block
/// 1 133 328, replayed in blocks:
///
/// 1 135 391
/// 1 144 468
pub const TX_BLOCK_1_133_328: [u64; 2] = [1_135_391, 1_144_468];

/// Transaction 0x3cc27e7cc8b7a9380b2b2f6c224ea5ef06ade62a6af564a9dd0bcca92131cd4e in block
/// 1 244 152, replayed in block:
///
/// 1 272 994
pub const TX_BLOCK_1_244_152: u64 = 1_272_994;

/// The six blocks with replayed transactions.
pub const BLOCK_NUMS_REPLAYED_TX: [u64; 6] = [
    TX_BLOCK_985[0],
    TX_BLOCK_985[1],
    TX_BLOCK_123_322,
    TX_BLOCK_1_133_328[0],
    TX_BLOCK_1_133_328[1],
    TX_BLOCK_1_244_152,
];

/// Returns `true` if transaction is the second or third appearance of the transaction. The blocks
/// with replayed transaction happen to only contain the single transaction.
pub fn is_dup_tx(block_number: u64) -> bool {
    if block_number > BLOCK_NUMS_REPLAYED_TX[5] {
        return false
    }

    // these blocks just have one transaction!
    if BLOCK_NUMS_REPLAYED_TX.contains(&block_number) {
        return true
    }

    false
}

/// Last OVM Header hash on Optimism Mainnet.
pub const LAST_OVM_HEADER_HASH: B256 =
    b256!("21a168dfa5e727926063a28ba16fd5ee84c814e847c81a699c7a0ea551e4ca50");

/// Last OVM Header on Optimism Mainnet. (`105_235_062`)
pub const LAST_OVM_HEADER: Header = Header {
    difficulty: U256::from_limbs([2u64, 0, 0, 0]),
    extra_data: bytes!("d98301090a846765746889676f312e31352e3133856c696e75780000000000006579f2174bcc83cf381ad2f867e287448bc3472d40fe92a35e77a6922cbd033b0fff6a2b60929856dfd7993bac8626cd57a77daf86247a47d4d8e90d5f653e9e01"),
    gas_limit: 0xe4e1c0,
    gas_used: 0x20e8d,
    logs_bloom: bloom!("00000000000000000000000000000000000000000000000000000000001000000000000000000080000000000000000100000000000000000000004000000000008000000024000000000008000000000000000000000000000000000000002000000000020000400000000000020800200000200000400000000018000000004000000000000000000000000000000001800000000000000000000000000000000080040000000000000000000000000000a00000000000000000000000004000000002000000000200000000000000000002120000000000000000000020000000000800000000000000000000000000000000000008000000000008000001"),
    nonce: 0,
    number: 0x645c276,
    parent_hash: b256!("d905ae955c0dafb7cdd1900e08289f6c3e939388c7b810e7de4eb441aabf9d59"),
    receipts_root: b256!("1e415689e75e50719c2d9714995cd9615edabffb3390ad908803ca21d2df96fb"),
    state_root: b256!("4adc41ce7ddde6398c47f5e2fbe3f31754268f12e4f14b2a575cf9d5b5363bd3"),
    timestamp: 0x647f587d,
    transactions_root: b256!("353169389d80e656f9da5866eb1b78d3397a3d947c71753194bbd1a8a6108076"),
    ommers_hash: EMPTY_OMMER_ROOT_HASH,
    beneficiary: Address::ZERO,
    withdrawals_root: None,
    mix_hash: B256::ZERO,
    base_fee_per_gas: None,
    blob_gas_used: None,
    excess_blob_gas: None,
    parent_beacon_block_root: None,
    requests_root: None,
};

/// Last OVM Header total difficulty on Optimism Mainnet.
pub const LAST_OVM_HEADER_TTD: U256 = U256::from_limbs([210470125u64, 0, 0, 0]);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_last_ovm_header_hash() {
        assert_eq!(LAST_OVM_HEADER.hash_slow(), LAST_OVM_HEADER_HASH);
    }
}
