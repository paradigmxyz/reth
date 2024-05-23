//! Replayed OP mainnet OVM transactions (in blocks below Bedrock).

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
