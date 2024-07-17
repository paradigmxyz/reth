use reth_primitives::{Header, U256};
use std::ops::Shr;

/// Bound divisor of the difficulty (2048)
/// This constant is the right-shifts to use for the division.
const PARENT_DIFF_SHIFT: usize = 11;

/// Exponential difficulty period
const EXP_DIFF_PERIOD_UINT: u64 = 100_000;

// FRONTIER_DURATION_LIMIT is for Frontier:
// The decision boundary on the blocktime duration used to determine
// whether difficulty should go up or down.
const FRONTIER_DURATION_LIMIT: u64 = 13;

// MINIMUM_DIFFICULTY The minimum that the difficulty may ever be.
const MINIMUM_DIFFICULTY: U256 = U256::from(131072);

/// Bomb delays for different EIPS
pub enum BombDelay {
    Byzantium = 3_000_000,
    Constantinople = 5_000_000,
    Eip2384 = 9_000_000,
    Eip3554 = 9_700_000,
    Eip4345 = 10_700_000,
    Eip5133 = 11_400_000,
}
/// calc_difficulty_frontier is the difficulty adjustment algorithm. It returns the
/// difficulty that a new block should have when created at time given the parent
/// block's time and difficulty. The calculation uses the Frontier rules.
///
/// ## Algorithm
/// block_diff = pdiff + pdiff / 2048 * (1 if time - ptime < 13 else -1) + int(2^((num // 100000) -
/// 2))
///
/// Where:
/// - pdiff  = parent.difficulty
/// - ptime = parent.time
/// - time = block.timestamp
/// - num = block.number
fn calc_difficulty_frontier(timestamp: u64, parent: &Header) -> Result<U256, ()> {
    // Children block timestamp should be later than parent block timestamp
    assert!(timestamp > parent.timestamp);

    // Get number of the current block
    let block_num = parent.number + 1;

    // pdiff_adj = pdiff / 2048
    let mut pdiff = parent.difficulty.shr(PARENT_DIFF_SHIFT);

    // pdiff = pdiff + pdiff / 2048 * (1 if time - ptime < 13 else -1)
    if timestamp - parent.timestamp < FRONTIER_DURATION_LIMIT {
        pdiff = parent.difficulty.checked_add(pdiff).ok_or(Err(()))?;
    } else {
        pdiff = parent.difficulty.checked_sub(pdiff).ok_or(Err(()))?;
    }

    // int(2^((num // 100000) - 2))
    let exponent = (block_num / EXP_DIFF_PERIOD_UINT);
    if exponent > 1 {
        let exponent = exponent.saturating_sub(2).try_into()?;
        let period_count = U256::from(1).checked_shl(exponent).ok_or(Err(()))?;
        pdiff = pdiff + period_count;
    }
    // block_diff = pdiff + pdiff / 2048 * (1 if time - ptime < 13 else -1) + int(2^((num // 100000)
    // - 2))
    Ok(pdiff)
}

/// Difficulty adjustment algorithm. It returns
/// the difficulty that a new block should have when created at time given the
/// parent block's time and difficulty. The calculation uses the Homestead rules.
///
/// ## Algorithm
///
/// Source:
/// block_diff = pdiff + pdiff / 2048 * max(1 - (time - ptime) / 10, -99) + 2 ^ int((num / 100000) -
/// 2))
///
/// Current modification to use unsigned ints:
/// block_diff = pdiff - pdiff / 2048 * max((time - ptime) / 10 - 1, 99) + 2 ^ int((num / 100000) -
/// 2))
///
/// Where:
/// - pdiff  = parent.difficulty
/// - ptime = parent.time
/// - time = block.timestamp
/// - num = block.number
fn calc_difficulty_homestead(timestamp: u64, parent: &Header) -> Result<U256, ()> {
    // Children block timestamp should be later than parent block timestamp
    assert!(timestamp > parent.timestamp);

    // Get number of the current block
    let block_num = parent.number + 1;

    let mut pdiff = parent.difficulty.shr(PARENT_DIFF_SHIFT);
    let mut time_adj = (timestamp - parent.timestamp) / 10;
    let mut negative = true;

    time_adj = match time_adj {
        0 => {
            negative = false;
            1
        }
        100.. => 99,
        _ => time_adj - 1,
    };

    pdiff = pdiff.checked_mul(U256::from(time_adj)).ok_or(Err(()))?;

    if negative {
        pdiff = pdiff.checked_sub(U256::from(1)).ok_or(Err(()))?;
    } else {
        pdiff = pdiff.checked_add(U256::from(1)).ok_or(Err(()))?;
    }

    if pdiff < MINIMUM_DIFFICULTY {
        pdiff = MINIMUM_DIFFICULTY;
    }

    let exponent = (block_num / EXP_DIFF_PERIOD_UINT);
    if exponent > 1 {
        let exponent = exponent.saturating_sub(2).try_into()?;
        let period_count = U256::from(1).checked_shl(exponent).ok_or(Err(()))?;
        pdiff = pdiff + period_count;
    }

    Ok(pdiff)
}

/// calc_difficulty_generic calculates the difficulty with Byzantium rules, which differs from Homestead in
/// how uncles affect the calculation.
///
/// ## Algorithm
///
/// https://github.com/ethereum/EIPs/issues/100
/// adj_factor = max((2 if len(parent.uncles) else 1) - ((timestamp - parent.timestamp) // 9), -99)
/// child_diff = int(max(parent.difficulty + (parent.difficulty // BLOCK_DIFF_FACTOR) * adj_factor, min(parent.difficulty, MIN_DIFF)))
fn calc_difficulty_generic(
    timestamp: u64,
    parent: &Header,
    bomb_delay: &BombDelay,
) -> Result<U256, ()> {
    /*
        https://github.com/ethereum/EIPs/issues/100
        pDiff = parent.difficulty
        BLOCK_DIFF_FACTOR = 9
        adj_factor = max((2 if len(parent.uncles) else 1) - ((timestamp - parent.timestamp) // 9), -99)
        a = pDiff + (pDiff // BLOCK_DIFF_FACTOR) * adj_factor
        b = min(parent.difficulty, MIN_DIFF)
        child_diff = max(a,b )
    */
    // Children block timestamp should be later than parent block timestamp
    assert!(timestamp > parent.timestamp);

    // Note, the calculations below looks at the parent number, which is 1 below
    // the block number. Thus, we remove one from the delay given
    let bomb_delay_from_parent = u64::from(bomb_delay) - 1;

    let mut time_adj = (timestamp - parent.timestamp) / 9;
    let uncle_adj = if parent.ommers_hash_is_empty() { 1 } else { 2 };

    let negative = time_adj >= uncle_adj;
    if negative {
        time_adj = time_adj - uncle_adj;
    } else {
        time_adj = uncle_adj - time_adj;
    }
    if time_adj > 99 {
        time_adj = 99;
    }

    let mut pdiff = parent.difficulty.shr(PARENT_DIFF_SHIFT);

    pdiff = pdiff.checked_mul(U256::from(time_adj)).ok_or(Err(()))?;

    if negative {
        pdiff = parent.difficulty.checked_sub(pdiff).ok_or(Err(()))?;
    } else {
        pdiff = parent.difficulty.checked_add(pdiff).ok_or(Err(()))?;
    }

    if pdiff < MINIMUM_DIFFICULTY {
        pdiff = MINIMUM_DIFFICULTY;
    }

    if bomb_delay_from_parent > parent.number {
        let fake_block_num = parent.number - bomb_delay_from_parent;
        if fake_block_num >= 2 * EXP_DIFF_PERIOD_UINT {
            let mut exponent = (fake_block_num / EXP_DIFF_PERIOD_UINT);
            let exponent = exponent.saturating_sub(2).try_into()?;
            let period_count = U256::from(1).checked_shl(exponent).ok_or(Err(()))?;
            pdiff = pdiff.checked_add(period_count).ok_or(Err(()))?;
        }
    }

    Ok(pdiff)
}
