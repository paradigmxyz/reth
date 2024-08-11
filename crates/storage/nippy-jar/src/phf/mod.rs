use crate::NippyJarError;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, hash::Hash};

mod fmph;
pub use fmph::Fmph;

mod go_fmph;
pub use go_fmph::GoFmph;

pub trait PHFKey: AsRef<[u8]> + Sync + Clone + Hash {}
impl<T: AsRef<[u8]> + Sync + Clone + Hash> PHFKey for T {}

#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DummyFunction {
    map: HashMap<Vec<u8>, usize>,
}
#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DummyGoFunction {
    pub(crate) map: HashMap<Vec<u8>, usize>,
}

impl<T: PHFKey> From<&[T]> for DummyFunction {
    fn from(keys: &[T]) -> Self {
        let mut map = HashMap::new();
        for (i, key) in keys.iter().enumerate() {
            map.insert(key.as_ref().to_vec(), i);
        }
        Self { map }
    }
}

impl DummyFunction {
    pub(crate) fn get(&self, key: &[u8]) -> Option<u64> {
        self.map.get(key).map(|&v| v as u64)
    }
}

impl<T: PHFKey> From<&[T]> for DummyGoFunction {
    fn from(keys: &[T]) -> Self {
        let mut map = HashMap::new();
        for (i, key) in keys.iter().enumerate() {
            map.insert(key.as_ref().to_vec(), i);
        }
        Self { map }
    }
}

impl DummyGoFunction {
    pub(crate) fn get(&self, key: &[u8]) -> Option<u64> {
        self.map.get(key).map(|&v| v as u64)
    }
}

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
            Self::Fmph(f) => f.set_keys(keys),
            Self::GoFmph(f) => f.set_keys(keys),
        }
    }

    fn get_index(&self, key: &[u8]) -> Result<Option<u64>, NippyJarError> {
        match self {
            Self::Fmph(f) => f.get_index(key),
            Self::GoFmph(f) => f.get_index(key),
        }
    }
}
