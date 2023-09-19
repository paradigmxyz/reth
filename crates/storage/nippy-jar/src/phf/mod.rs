use crate::NippyJarError;
use serde::{Deserialize, Serialize};
use std::{clone::Clone, hash::Hash, marker::Sync};

mod fmph;
pub use fmph::Fmph;

mod go_fmph;
pub use go_fmph::GoFmph;

/// Trait to build and query a perfect hashing function.
pub trait KeySet {
    /// Adds the key set and builds the perfect hashing function.
    fn set_keys<T: AsRef<[u8]> + Sync + Clone + Hash>(
        &mut self,
        keys: &[T],
    ) -> Result<(), NippyJarError>;

    /// Get corresponding key index.
    fn get_index(&self, key: &[u8]) -> Result<Option<u64>, NippyJarError>;
}

/// Enumerates all types of perfect hashing functions.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum Functions {
    Fmph(Fmph),
    GoFmph(GoFmph),
}

impl KeySet for Functions {
    fn set_keys<T: AsRef<[u8]> + Sync + Clone + Hash>(
        &mut self,
        keys: &[T],
    ) -> Result<(), NippyJarError> {
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
