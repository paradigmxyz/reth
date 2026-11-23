//! Finite sparse-trie coverage for instrumented fuzz builds.
//!
//! Each bucket has a non-inlined body with a distinct black-boxed ID so coverage
//! instrumentation can distinguish it after optimization. Keep LTO disabled when collecting
//! sanitizer coverage. The bank contains 28,672 local-neighborhood and two control buckets.

use core::hint::black_box;

const NO_PARENT: usize = 28672;
const INVALID_PROOF: usize = NO_PARENT + 1;
const ARM_COUNT: usize = INVALID_PROOF + 1;

#[inline(never)]
fn coverage_arm<const ID: usize>() {
    // The distinct immediate keeps optimized bodies separate without a shared-memory write.
    black_box(ID);
}

// Double a contiguous list of IDs for each binary digit, appending one for a set digit.
// Expanding expressions rather than const-loop assignments gives each pointer its own
// const-generic instantiation on stable Rust, without generated source or a build script.
macro_rules! coverage_arms {
    (@expand [$($id:expr,)*]; $size:expr;) => {
        [$(coverage_arm::<{ $id }> as fn(),)*]
    };
    (@expand [$($id:expr,)*]; $size:expr; 0 $($rest:tt)*) => {
        coverage_arms!(@expand [$($id,)* $($id + $size,)*]; $size * 2; $($rest)*)
    };
    (@expand [$($id:expr,)*]; $size:expr; 1 $($rest:tt)*) => {
        coverage_arms!(@expand [$($id,)* $($id + $size,)* $size * 2,];
            $size * 2 + 1; $($rest)*)
    };
    (1 $($rest:tt)*) => {
        coverage_arms!(@expand [0,]; 1; $($rest)*)
    };
}

// 28674 in binary; the array length also checks the expansion's size.
static ARMS: [fn(); ARM_COUNT] = coverage_arms!(1 1 1 0 0 0 0 0 0 0 0 0 0 1 0);

struct Observation {
    branch_path_len: usize,
    state_mask: u16,
    revealed_mask: u16,
    leaf_mask: u16,
}

impl Observation {
    const fn valid(&self) -> bool {
        self.branch_path_len < 64 &&
            self.revealed_mask & !self.state_mask == 0 &&
            self.leaf_mask & !self.revealed_mask == 0
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
    (usize::from(depth_bucket(observation.branch_path_len)) * 16 + usize::from(target)) * 256 +
        pattern
}

/// Observes a blinded proof target at sparse-trie proof emission, never at the caller/model.
///
/// Masks describe its local revealed logical parent. A blinded root has no parent and
/// passes zero masks/depth/target; it hits a separate bounded control, not the local bank.
pub(super) fn observe_proof_target(
    branch_path_len: usize,
    has_parent: bool,
    target: u8,
    state_mask: u16,
    revealed_mask: u16,
    leaf_mask: u16,
) {
    let observation = Observation { branch_path_len, state_mask, revealed_mask, leaf_mask };
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
}
