use reth_primitives::alloy_primitives::utils::ParseUnits::U256;
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
const MINIMUM_DIFFICULTY: U256 = 131072;

const U256_ONE: U256 = 1;

/// calc_difficulty_frontier is the difficulty adjustment algorithm. It returns the
/// difficulty that a new block should have when created at time given the parent
/// block's time and difficulty. The calculation uses the Frontier rules.
///
/// ## Algorithm
/// block_diff = pdiff + pdiff / 2048 * (1 if time - ptime < 13 else -1) + int(2^((num // 100000) - 2))
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
        let exponent = U256::from(exponent.saturating_sub(2));
        let period_count = U256::from(1).checked_shl(exponent).ok_or(Err(()))?;
        pdiff = pdiff + period_count;
    }
    // block_diff = pdiff + pdiff / 2048 * (1 if time - ptime < 13 else -1) + int(2^((num // 100000) - 2))
    Ok(pdiff)
}


/// Difficulty adjustment algorithm. It returns
/// the difficulty that a new block should have when created at time given the
/// parent block's time and difficulty. The calculation uses the Homestead rules.
///
/// ## Algorithm
///
/// Source:
/// block_diff = pdiff + pdiff / 2048 * max(1 - (time - ptime) / 10, -99) + 2 ^ int((num / 100000) - 2))
///
/// Current modification to use unsigned ints:
/// block_diff = pdiff - pdiff / 2048 * max((time - ptime) / 10 - 1, 99) + 2 ^ int((num / 100000) - 2))
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
        let exponent = U256::from(exponent.saturating_sub(2));
        let period_count = U256::from(1).checked_shl(exponent).ok_or(Err(()))?;
        pdiff = pdiff + period_count;
    }

    Ok(pdiff)
}

// fn calc_difficulty_generic(bomb_delay: i64) {
//     /*
//         https://github.com/ethereum/EIPs/issues/100
//         pDiff = parent.difficulty
//         BLOCK_DIFF_FACTOR = 9
//         adj_factor = max((2 if len(parent.uncles) else 1) - ((timestamp - parent.timestamp) // 9), -99)
//         a = pDiff + (pDiff // BLOCK_DIFF_FACTOR) * adj_factor
//         b = min(parent.difficulty, MIN_DIFF)
//         child_diff = max(a,b )
//     */
//
//     let bomb_delay_from_parent = bomb_delay.saturating_sub(1);
//
//     // https://github.com/ethereum/EIPs/issues/100.
//     // algorithm:
//     // diff = (parent_diff +
//     //         (parent_diff / 2048 * max((2 if len(parent.uncles) else 1) - ((timestamp - parent.timestamp) // 9), -99))
//     //        ) + 2^(periodCount - 2)
// }

// fn calculate_difficulty(timestamp: u64, parent: &Header, bomb_delay_from_parent: u64) {
//     let block_time = timestamp;
//     let parent_time = parent.timestamp;
//
//     assert!(block_time > parent_time);
//
//     // timestamp_adj = ((timestamp - parent.timestamp) // 9)
//     let timestamp_adj = (block_time - parent_time) / 9;
//
//     // pdiff_adj = (parent_diff / 2048)
//     let pdiff_adj = parent.difficulty.shr(PARENT_DIFF_SHIFT);
//
//     // uncles_adj = (2 if len(parent.uncles) else 1)
//     let uncles_adj: u64 = if parent.ommers_hash_is_empty() { 1 } else { 2 };
//
//     // adj_factor = max((2 if len(parent.uncles) else 1) - ((timestamp - parent.timestamp) // 9), -99)
//     // adj_factor = max(uncles_adj - timestamp_adj, -99)
//     // casting u64 to i128 is safe
//     let mut adj_factor = (uncles_adj.wrapping_sub(timestamp_adj)).max(-99);
//
//     if uncles_adj < timestamp_adj {
//     } else {
//     }
// }
