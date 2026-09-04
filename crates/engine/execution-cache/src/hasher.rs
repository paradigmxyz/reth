//! Hash selection for replayable fixed-cache collision eviction.

use alloy_primitives::map::foldhash::fast::{FixedState, FoldHasher};
use std::hash::{BuildHasher, Hash, Hasher};

/// Keeps the native hash builder unless deterministic cache construction is requested.
pub(super) enum CacheHashBuilder<H> {
    Native(H),
    Deterministic,
}

impl<H: Default> CacheHashBuilder<H> {
    pub(super) fn new(deterministic: bool) -> Self {
        if deterministic {
            Self::Deterministic
        } else {
            Self::Native(H::default())
        }
    }
}

impl<H: BuildHasher> BuildHasher for CacheHashBuilder<H> {
    type Hasher = CacheHasher<H::Hasher>;

    fn build_hasher(&self) -> Self::Hasher {
        match self {
            Self::Native(hasher) => CacheHasher::Native(hasher.build_hasher()),
            Self::Deterministic => CacheHasher::Deterministic(FixedState::default().build_hasher()),
        }
    }

    // fixed-cache hashes each key in one call, so the native path retains the original hasher's
    // optimized implementation instead of dispatching every individual write through the enum.
    #[inline]
    fn hash_one<T: Hash>(&self, value: T) -> u64 {
        match self {
            Self::Native(hasher) => hasher.hash_one(value),
            Self::Deterministic => FixedState::default().hash_one(value),
        }
    }
}

pub(super) enum CacheHasher<H> {
    Native(H),
    Deterministic(FoldHasher<'static>),
}

macro_rules! forward_writes {
    ($( $method:ident($ty:ty) ),* $(,)?) => {$(
        fn $method(&mut self, value: $ty) {
            match self {
                Self::Native(hasher) => hasher.$method(value),
                Self::Deterministic(hasher) => hasher.$method(value),
            }
        }
    )*};
}

impl<H: Hasher> Hasher for CacheHasher<H> {
    forward_writes! {
        write(&[u8]),
        write_u8(u8),
        write_u16(u16),
        write_u32(u32),
        write_u64(u64),
        write_u128(u128),
        write_usize(usize),
        write_i8(i8),
        write_i16(i16),
        write_i32(i32),
        write_i64(i64),
        write_i128(i128),
        write_isize(isize),
    }

    fn finish(&self) -> u64 {
        match self {
            Self::Native(hasher) => hasher.finish(),
            Self::Deterministic(hasher) => hasher.finish(),
        }
    }
}
