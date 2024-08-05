use reth_primitives::{alloy_primitives::BlockTimestamp, Header, U256};

/// Exponential difficulty period
const EXP_DIFF_PERIOD_UINT: u64 = 100_000;

/// The minimum that the difficulty may ever be.
const MINIMUM_DIFFICULTY: u64 = 131_072;

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
///
/// Reference:
/// - https://github.com/ethereum/EIPs/issues/100
pub fn make_difficulty_calculator(
    block_timestamp: BlockTimestamp,
    parent: &Header,
    bomb_delay: &BombDelay,
) -> Result<U256, ()> {
    assert!(block_timestamp > parent.timestamp);

    // Note, the calculations below looks at the parent number, which is 1 below
    // the block number. Thus, we remove one from the delay given
    let bomb_delay_from_parent = u64::from(bomb_delay).saturating_sub(1);

    let diff_time = block_timestamp - parent.timestamp;
    let uncles = if parent.ommers_hash_is_empty() { 1 } else { 2 };

    let factor = uncles + diff_time / 9;

    let difficulty = u64::try_from(parent.difficulty).unwrap();
    let left_side = difficulty / 2048;

    let mut calculated_difficulty =
        difficulty + left_side * std::cmp::max(factor, u64::try_from(99).unwrap());

    if calculated_difficulty < MINIMUM_DIFFICULTY {
        calculated_difficulty = MINIMUM_DIFFICULTY;
    }

    if parent.number >= bomb_delay_from_parent {
        let fake_block = parent.number - bomb_delay_from_parent;

        let period_count = fake_block / EXP_DIFF_PERIOD_UINT;

        if period_count > 1 {
            let pow_period_count = (period_count - 2).pow(2);

            calculated_difficulty += pow_period_count;
        }
    }

    Ok(U256::from(calculated_difficulty))
}
