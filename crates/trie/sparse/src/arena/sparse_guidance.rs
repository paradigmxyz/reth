//! Finite sparse-trie coverage for instrumented fuzz builds.
//!
//! Each bucket has a non-inlined body with a distinct atomic counter address so coverage
//! instrumentation can distinguish it after optimization. Keep LTO disabled when collecting
//! sanitizer coverage. Regenerate the bank with `python3
//! crates/trie/sparse/generate_sparse_buckets.py`. The bank contains 544 encoder, 28,672 local,
//! 15,680 sandwich and two control buckets.

use core::{
    hint::black_box,
    sync::atomic::{AtomicU64, Ordering},
};

static COUNTERS: [AtomicU64; ARM_COUNT] = [const { AtomicU64::new(0) }; ARM_COUNT];

// Index the finite exact-triple bank without scanning all 1,120 triples at every request.
static SANDWICH_INDEX: [[[usize; 16]; 16]; 16] = {
    let mut table = [[[usize::MAX; 16]; 16]; 16];
    let mut rank = 0;
    let mut i = 0;
    while i < 16 {
        let mut j = 0;
        while j < 16 {
            let mut k = 0;
            while k < 16 {
                if (i < j && j < k) || (k < j && j < i) {
                    table[i][j][k] = rank;
                    rank += 1;
                }
                k += 1;
            }
            j += 1;
        }
        i += 1;
    }
    table
};

macro_rules! coverage_arm {
    ($name:ident, $index:literal) => {
        #[inline(never)]
        fn $name() {
            // A different address and immediate in every body prevent identical-code folding.
            let counter = black_box(&COUNTERS[$index]);
            let previous = counter.fetch_or(1, Ordering::Relaxed);
            black_box(($index, previous));
        }
    };
}

include!("sparse_buckets_generated.rs");

struct Observation {
    branch_path_len: usize,
    is_root: bool,
    state_mask: u16,
    revealed_mask: u16,
    leaf_mask: u16,
}

impl Observation {
    const fn masks(&self) -> [u16; 5] {
        [
            self.state_mask,
            self.revealed_mask,
            self.leaf_mask,
            self.state_mask & !self.revealed_mask,
            self.revealed_mask & !self.leaf_mask,
        ]
    }

    const fn valid(&self) -> bool {
        self.branch_path_len < 64 &&
            self.is_root == (self.branch_path_len == 0) &&
            self.revealed_mask & !self.state_mask == 0 &&
            self.leaf_mask & !self.revealed_mask == 0
    }
}

const fn count_bucket(mask: u16) -> u8 {
    match mask.count_ones() {
        0 => 0,
        1 => 1,
        2..=4 => 2,
        5..=8 => 3,
        _ => 4,
    }
}

fn shape_bucket(mask: u16) -> u8 {
    match mask {
        0 => 0,
        u16::MAX => 4,
        _ if mask.is_power_of_two() => 1,
        _ => {
            let shifted = u32::from(mask) >> mask.trailing_zeros();
            if shifted & (shifted + 1) == 0 {
                2
            } else {
                3
            }
        }
    }
}

const fn depth_bucket(depth: usize) -> u8 {
    match depth {
        0 => 0,
        1 => 1,
        2..=4 => 2,
        5..=8 => 3,
        9..=16 => 4,
        17..=32 => 5,
        _ => 6,
    }
}

/// Receives one branch at RLP encoding in the live arena.
///
/// State means occupied children; revealed means arena-backed; leaf means a revealed
/// leaf, including lower-subtrie leaf roots. All masks use nibble positions.
pub(super) fn observe_branch_shape(
    branch_path_len: usize,
    is_root: bool,
    state_mask: u16,
    revealed_mask: u16,
    leaf_mask: u16,
) {
    let observation =
        Observation { branch_path_len, is_root, state_mask, revealed_mask, leaf_mask };
    if !observation.valid() {
        coverage_invalid_observation();
        return;
    }
    dispatch(&observation);
}

fn local_index(observation: &Observation, target: u8) -> usize {
    let mut pattern = 0;
    // No wrapping: out-of-range positions are absent; target nibble remains a dimension.
    for (slot, delta) in [-2i16, -1, 1, 2].into_iter().enumerate() {
        let nibble = i16::from(target) + delta;
        let kind = if (0..16).contains(&nibble) {
            let bit = 1u16 << nibble;
            if observation.state_mask & bit == 0 {
                0
            } else if observation.revealed_mask & bit == 0 {
                1
            } else if observation.leaf_mask & bit == 0 {
                2
            } else {
                3
            }
        } else {
            0
        };
        pattern |= kind << (slot * 2);
    }
    LOCAL_START +
        (usize::from(depth_bucket(observation.branch_path_len)) * 16 + usize::from(target)) * 256 +
        pattern
}

/// Observes a blinded proof target at sparse-trie proof emission, never at the caller/model.
///
/// Masks describe its local revealed logical parent. A blinded root has no parent and
/// passes zero masks/depth/target; it hits a separate bounded control, not either bank.
pub(super) fn observe_proof_target(
    branch_path_len: usize,
    has_parent: bool,
    target: u8,
    state_mask: u16,
    revealed_mask: u16,
    leaf_mask: u16,
) {
    let observation = Observation {
        branch_path_len,
        is_root: branch_path_len == 0,
        state_mask,
        revealed_mask,
        leaf_mask,
    };
    if !has_parent {
        if (branch_path_len, target, state_mask, revealed_mask, leaf_mask) == (0, 0, 0, 0, 0) {
            ARMS[NO_PARENT]();
        } else {
            ARMS[INVALID_PROOF]();
        }
        return;
    }
    if !observation.valid() ||
        target >= 16 ||
        state_mask & (1 << target) == 0 ||
        revealed_mask & (1 << target) != 0
    {
        ARMS[INVALID_PROOF]();
        return;
    }
    ARMS[local_index(&observation, target)]();
    let depth = usize::from(depth_bucket(branch_path_len));
    for (j, ranks) in SANDWICH_INDEX[usize::from(target)].iter().enumerate() {
        if revealed_mask & (1 << j) == 0 {
            continue;
        }
        for (k, &rank) in ranks.iter().enumerate() {
            if state_mask & !revealed_mask & (1 << k) == 0 {
                continue;
            }
            if rank != usize::MAX {
                let leaf = usize::from(leaf_mask & (1 << j) != 0);
                ARMS[SANDWICH_START + (depth * 1120 + rank) * 2 + leaf]();
            }
        }
    }
}
