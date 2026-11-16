//! Finite sparse-trie coverage for instrumented fuzz builds.
//!
//! Each bucket has a non-inlined body with a distinct atomic counter address so coverage
//! instrumentation can distinguish it after optimization. Keep LTO disabled when collecting
//! sanitizer coverage. The bank contains 544 encoder, 28,672 local,
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

const ENCODER_COUNT: usize = 544;
const INVALID_OBSERVATION: usize = ENCODER_COUNT - 1;
const LOCAL_START: usize = ENCODER_COUNT;
const SANDWICH_START: usize = LOCAL_START + 28672;
const NO_PARENT: usize = SANDWICH_START + 15680;
const INVALID_PROOF: usize = NO_PARENT + 1;
const ARM_COUNT: usize = INVALID_PROOF + 1;

#[inline(never)]
fn coverage_arm<const ID: usize>() {
    // A different address and immediate in every body prevent identical-code folding.
    let counter = black_box(&COUNTERS[ID]);
    let previous = counter.fetch_or(1, Ordering::Relaxed);
    black_box((ID, previous));
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

// 44898 in binary; the array length also checks the expansion's size.
static ARMS: [fn(); ARM_COUNT] = coverage_arms!(1 0 1 0 1 1 1 1 0 1 1 0 0 0 1 0);

// Every encoder family has a contiguous range in the stable bucket numbering.
macro_rules! coverage_families {
    ($($start:expr => $rank:expr),+ $(,)?) => {
        $(ARMS[$rank.map_or(INVALID_OBSERVATION, |rank| $start + rank)]();)+
    };
}

const fn count_shape_rank(count: usize, shape: usize) -> Option<usize> {
    match (count, shape) {
        (0, 0) => Some(0),
        (1, 1) => Some(1),
        (2, 2) => Some(2),
        (2, 3) => Some(3),
        (3, 2) => Some(4),
        (3, 3) => Some(5),
        (4, 2) => Some(6),
        (4, 3) => Some(7),
        (4, 4) => Some(8),
        _ => None,
    }
}

fn subset_count_rank(count: usize, subset: usize) -> Option<usize> {
    (subset <= count).then_some(count * (count + 1) / 2 + subset)
}

fn dispatch(observation: &Observation) {
    let depth = usize::from(depth_bucket(observation.branch_path_len));
    let root = usize::from(observation.is_root);
    let masks = observation.masks();
    let counts = masks.map(|mask| usize::from(count_bucket(mask)));
    let shapes = masks.map(|mask| usize::from(shape_bucket(mask)));
    let count_shapes = core::array::from_fn::<_, 5, _>(|i| count_shape_rank(counts[i], shapes[i]));
    coverage_families! { 0 => Some(depth), 7 => Some(root) }
    for i in 0..5 {
        coverage_families! {
            9 + i * 19 => Some(counts[i]),
            14 + i * 19 => Some(shapes[i]),
            19 + i * 19 => count_shapes[i],
        }
    }
    for (i, rank) in count_shapes.into_iter().enumerate() {
        coverage_families! { 104 + i * 63 => rank.map(|rank| depth * 9 + rank) }
    }
    coverage_families! {
        419 => subset_count_rank(counts[0], counts[1]),
        434 => subset_count_rank(counts[1], counts[2]),
        449 => (counts[3] != 4 || counts[4] != 4).then_some(counts[3] * 5 + counts[4]),
        // Lexicographic triples with leaf <= revealed <= state, 35 per root kind.
        473 => (counts[2] <= counts[1] && counts[1] <= counts[0]).then_some(
            root * 35 + counts[0] * (counts[0] + 1) * (counts[0] + 2) / 6 +
                counts[1] * (counts[1] + 1) / 2 + counts[2]
        ),
    }
}

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
        ARMS[INVALID_OBSERVATION]();
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
