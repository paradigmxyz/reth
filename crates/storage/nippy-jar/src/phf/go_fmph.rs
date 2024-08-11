use crate::{
    phf::{DummyGoFunction, PHFKey},
    NippyJarError, PerfectHashingFunction,
};
use serde::{Deserialize, Serialize};

/// Wrapper struct for [`DummyGoFunction`].
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct GoFmph {
    function: Option<DummyGoFunction>,
}

impl GoFmph {
    pub const fn new() -> Self {
        Self { function: None }
    }
}

impl PerfectHashingFunction for GoFmph {
    fn set_keys<T: PHFKey>(&mut self, keys: &[T]) -> Result<(), NippyJarError> {
        self.function = Some(DummyGoFunction::from(keys));
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
impl PartialEq for GoFmph {
    fn eq(&self, other: &Self) -> bool {
        match (&self.function, &other.function) {
            (Some(func1), Some(func2)) => func1 == func2,
            _ => false,
        }
    }
}
