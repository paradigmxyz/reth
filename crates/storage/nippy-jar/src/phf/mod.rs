use crate::NippyJarError;
use serde::{Deserialize, Serialize};
use std::hash::Hash;

mod fmph;
pub use fmph::Fmph;

mod go_fmph;
pub use go_fmph::GoFmph;

/// Trait alias for [`PerfectHashingFunction`] keys.
pub trait PHFKey: AsRef<[u8]> + Sync + Clone + Hash {}
impl<T: AsRef<[u8]> + Sync + Clone + Hash> PHFKey for T {}

/// Trait to build and query a perfect hashing function.
pub trait PerfectHashingFunction: Serialize + for<'a> Deserialize<'a> {
    /// Adds the key set and builds the perfect hashing function.
    fn set_keys<T: PHFKey>(&mut self, keys: &[T]) -> Result<(), NippyJarError>;

    /// Get corresponding associated integer. There might be false positives.
    fn get_index(&self, key: &[u8]) -> Result<Option<u64>, NippyJarError>;
}

/// Enumerates all types of perfect hashing functions.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub enum Functions {
    Fmph(Fmph),
    GoFmph(GoFmph),
}

impl PerfectHashingFunction for Functions {
    fn set_keys<T: PHFKey>(&mut self, keys: &[T]) -> Result<(), NippyJarError> {
        match self {
            Functions::Fmph(f) => f.set_keys(keys),
            Functions::GoFmph(f) => f.set_keys(keys),
        }
    }

    fn get_index(&self, key: &[u8]) -> Result<Option<u64>, NippyJarError> {
        match self {
            Functions::Fmph(f) => f.get_index(key),
            Functions::GoFmph(f) => f.get_index(key),
        }
    }
}
