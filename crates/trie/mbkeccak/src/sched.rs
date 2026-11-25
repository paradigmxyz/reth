//! Lane scheduler: streams messages through a W-lane interleaved permutation.

use crate::RATE;

const WORDS: usize = RATE / 8;

/// W interleaved Keccak states: `0[i][j]` is lane word `i` of message slot `j`.
#[repr(C, align(64))]
pub(crate) struct State<const W: usize>(pub(crate) [[u64; W]; 25]);

/// Block-count buckets used for longest-first feeding; the last one holds everything larger.
const BUCKETS: usize = 5;

#[inline(always)]
fn bucket(len: usize) -> usize {
    let b = len / RATE; // blocks - 1
    if b >= BUCKETS - 1 {
        BUCKETS - 1
    } else {
        b
    }
}

/// Batches larger than this are fed in input order: the drain at the end is negligible next to
/// the total, and scanning buckets would cost more than it saves.
const ORDERED_MIN: usize = 256;

/// Yields input indices from the largest block-count bucket to the smallest (small batches), or
/// in input order (large batches), without allocating.
pub(crate) struct Feeder<'a> {
    inputs: &'a [&'a [u8]],
    ordered: bool,
    bucket: usize,
    pos: usize,
    /// Number of inputs in each bucket not yet yielded (small batches only).
    left: [usize; BUCKETS],
}

impl<'a> Feeder<'a> {
    pub(crate) fn new(inputs: &'a [&'a [u8]]) -> Self {
        let mut left = [0; BUCKETS];
        let ordered = inputs.len() >= ORDERED_MIN;
        if !ordered {
            for i in inputs {
                left[bucket(i.len())] += 1;
            }
        }
        let mut f = Feeder { inputs, ordered, bucket: BUCKETS - 1, pos: 0, left };
        if !ordered {
            f.skip_empty();
        }
        f
    }

    #[inline]
    fn skip_empty(&mut self) {
        while self.bucket > 0 && self.left[self.bucket] == 0 {
            self.bucket -= 1;
            self.pos = 0;
        }
    }

    #[inline]
    pub(crate) fn next(&mut self) -> Option<usize> {
        if self.ordered {
            let i = self.pos;
            if i < self.inputs.len() {
                self.pos += 1;
                return Some(i);
            }
            return None;
        }
        if self.left[self.bucket] == 0 {
            return None;
        }
        while bucket(self.inputs[self.pos].len()) != self.bucket {
            self.pos += 1;
        }
        let i = self.pos;
        self.pos += 1;
        self.left[self.bucket] -= 1;
        if self.left[self.bucket] == 0 {
            self.skip_empty();
        }
        Some(i)
    }
}

const NONE: usize = usize::MAX;

#[inline(always)]
fn load(b: &[u8], w: usize) -> u64 {
    u64::from_le_bytes(b[w * 8..w * 8 + 8].try_into().unwrap())
}

/// Hashes `inputs` into `out` with a W-lane permutation.
///
/// # Safety
///
/// `perm` must be safe to call on this CPU.
pub(crate) unsafe fn run<const W: usize>(
    inputs: &[&[u8]],
    out: &mut [[u8; 32]],
    perm: unsafe fn(&mut State<W>),
) {
    debug_assert_eq!(inputs.len(), out.len());
    let mut st = State::<W>([[0u64; W]; 25]);
    let mut msg = [NONE; W];
    let mut off = [0usize; W];
    let mut last = [false; W];
    let mut feeder = Feeder::new(inputs);
    let mut pad = [0u8; RATE];
    loop {
        let mut active = 0;
        for j in 0..W {
            let fresh = msg[j] == NONE;
            if fresh {
                match feeder.next() {
                    Some(i) => {
                        msg[j] = i;
                        off[j] = 0;
                    }
                    None => continue,
                }
            }
            active += 1;
            let m = inputs[msg[j]];
            let rest = &m[off[j]..];
            let block: &[u8] = if rest.len() >= RATE {
                off[j] += RATE;
                rest
            } else {
                pad = [0u8; RATE];
                pad[..rest.len()].copy_from_slice(rest);
                pad[rest.len()] ^= 0x01;
                pad[RATE - 1] ^= 0x80;
                last[j] = true;
                &pad
            };
            if fresh {
                for w in 0..WORDS {
                    st.0[w][j] = load(block, w);
                }
                for w in WORDS..25 {
                    st.0[w][j] = 0;
                }
            } else {
                for w in 0..WORDS {
                    st.0[w][j] ^= load(block, w);
                }
            }
        }
        if active == 0 {
            break;
        }
        // SAFETY: the caller guarantees `perm` runs on this CPU.
        unsafe { perm(&mut st) };
        for j in 0..W {
            if last[j] {
                let o = &mut out[msg[j]];
                for w in 0..4 {
                    o[w * 8..w * 8 + 8].copy_from_slice(&st.0[w][j].to_le_bytes());
                }
                last[j] = false;
                msg[j] = NONE;
            }
        }
    }
}
