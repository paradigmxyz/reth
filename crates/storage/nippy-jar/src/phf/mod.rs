use crate::NippyJarError;
use serde::{Deserialize, Serialize};
use std::{clone::Clone, hash::Hash, marker::Sync};

mod fmph;
pub use fmph::Fmph;

pub trait KeySet {
    /// Add key to the list.
    fn set_keys<T: AsRef<[u8]> + Sync + Clone + Hash>(
        &mut self,
        keys: &[T],
    ) -> Result<(), NippyJarError>;

    /// Get key index.
    fn get_index(&self, key: &[u8]) -> Result<Option<u64>, NippyJarError>;
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum Functions {
    Fmph(Fmph),
    // GoFmph(GoFmph),
    //Avoids irrefutable let errors. Remove this after adding another one.
    Unused,
}

impl KeySet for Functions {
    fn set_keys<T: AsRef<[u8]> + Sync + Clone + Hash>(
        &mut self,
        keys: &[T],
    ) -> Result<(), NippyJarError> {
        match self {
            Functions::Fmph(f) => f.set_keys(keys),
            Functions::Unused => unreachable!(),
        }
    }
    fn get_index(&self, key: &[u8]) -> Result<Option<u64>, NippyJarError> {
        match self {
            Functions::Fmph(f) => f.get_index(key),
            Functions::Unused => unreachable!(),
        }
    }
}
