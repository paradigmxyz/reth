use crate::{phf::DummyFunction, NippyJarError, PerfectHashingFunction};

use crate::phf::PHFKey;
use serde::{Deserialize, Serialize};

/// Wrapper struct for [`DummyFunction`].
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct Fmph {
    function: Option<DummyFunction>,
}

impl Fmph {
    pub const fn new() -> Self {
        Self { function: None }
    }
}

impl PerfectHashingFunction for Fmph {
    fn set_keys<T: PHFKey>(&mut self, keys: &[T]) -> Result<(), NippyJarError> {
        self.function = Some(DummyFunction::from(keys));
        Ok(())
    }

    fn get_index(&self, key: &[u8]) -> Result<Option<u64>, NippyJarError> {
        if let Some(f) = &self.function {
            return Ok(f.get(key))
        }
        Err(NippyJarError::PHFMissingKeys)
    }
}

#[cfg(test)]
impl PartialEq for Fmph {
    fn eq(&self, _other: &Self) -> bool {
        match (&self.function, &_other.function) {
            (Some(func1), Some(func2)) => func1 == func2,
            _ => false,
        }
    }
}
