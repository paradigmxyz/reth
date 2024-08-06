use reth_primitives::{alloy_primitives::BlockTimestamp, Header, U256};

/// Exponential difficulty period
const EXP_DIFF_PERIOD_UINT: u64 = 100_000;

/// The minimum that the difficulty may ever be.
const MINIMUM_DIFFICULTY: i64 = 131_072;

/// Bomb delays
#[derive(Copy, Clone, Debug)]
pub enum BombDelay {
    /// For byzantines rules
    Byzantium = 3_000_000,
}

impl From<&BombDelay> for u64 {
    fn from(value: &BombDelay) -> Self {
        *value as u64
    }
}

/// Calculates the difficulty of a block according to the Byzantium rules of Ethereum.
/// This differs from the Homestead rules in how uncle blocks affect the difficulty calculation.
///
/// The difficulty adjustment algorithm used here is defined in [EIP-100](https://eips.ethereum.org/EIPS/eip-100).
/// new_diff = (
/// parent_diff +
/// (parent_diff // 2048 * max((2 if len(parent.uncles) else 1) - ((timestamp - parent.timestamp) //
/// 9), -99))
/// + 2 ** (periodCount - 2)
/// )
pub fn make_difficulty_calculator(
    block_timestamp: BlockTimestamp,
    parent: &Header,
    bomb_delay: &BombDelay,
) -> Result<U256, ()> {
    assert!(block_timestamp > parent.timestamp);

    // Note, the calculations below looks at the parent number, which is 1 below
    // the block number. Thus, we remove one from the delay given
    let bomb_delay_from_parent = u64::from(bomb_delay).saturating_sub(1);

    let uncles_factor = if parent.ommers_hash_is_empty() { 1 } else { 2 };
    let parent_diff = i64::try_from(parent.difficulty).unwrap();

    let mut timestamp_factor: i64 =
        (uncles_factor + (block_timestamp - parent.timestamp) / 9).try_into().unwrap();

    if timestamp_factor < -99 {
        timestamp_factor = -99;
    }

    let mut diff = parent_diff + parent_diff / 2048 * timestamp_factor;

    if diff < MINIMUM_DIFFICULTY.try_into().unwrap() {
        diff = MINIMUM_DIFFICULTY;
    }

    // Calculates a fake block number for the ice-age delay
    // Specification: https://eips.ethereum.org/EIPS/eip-1234
    if parent.number >= bomb_delay_from_parent {
        let fake_block_number = parent.number - bomb_delay_from_parent;
        let period_count = fake_block_number / EXP_DIFF_PERIOD_UINT;

        if period_count >= 1 {
            diff += (period_count - 2).pow(2) as i64;
        }
    }

    Ok(U256::from(diff))
}
