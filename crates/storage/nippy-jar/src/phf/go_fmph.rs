use crate::{NippyJarError, PHFKey, PerfectHashingFunction};
use ph::fmph::{GOBuildConf, GOFunction};
use serde::{
    de::Error as DeSerdeError, ser::Error as SerdeError, Deserialize, Deserializer, Serialize,
    Serializer,
};

/// Wrapper struct for [`GOFunction`]. Implementation of the following [paper](https://dl.acm.org/doi/10.1145/3596453).
#[derive(Default)]
pub struct GoFmph {
    function: Option<GOFunction>,
}

impl GoFmph {
    pub const fn new() -> Self {
        Self { function: None }
    }
}

impl PerfectHashingFunction for GoFmph {
    fn set_keys<T: PHFKey>(&mut self, keys: &[T]) -> Result<(), NippyJarError> {
        self.function = Some(GOFunction::from_slice_with_conf(
            keys,
            GOBuildConf { use_multiple_threads: true, ..Default::default() },
        ));
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
            (Some(func1), Some(func2)) => {
                func1.level_sizes() == func2.level_sizes() &&
                    func1.write_bytes() == func2.write_bytes() &&
                    {
                        let mut f1 = Vec::with_capacity(func1.write_bytes());
                        func1.write(&mut f1).expect("enough capacity");

                        let mut f2 = Vec::with_capacity(func2.write_bytes());
                        func2.write(&mut f2).expect("enough capacity");

                        f1 == f2
                    }
            }
            (None, None) => true,
            _ => false,
        }
    }
}

impl std::fmt::Debug for GoFmph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoFmph")
            .field("bytes_size", &self.function.as_ref().map(|f| f.write_bytes()))
            .finish_non_exhaustive()
    }
}

impl Serialize for GoFmph {
    /// Potentially expensive, but should be used only when creating the file.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.function {
            Some(f) => {
                let mut v = Vec::with_capacity(f.write_bytes());
                f.write(&mut v).map_err(S::Error::custom)?;
                serializer.serialize_some(&v)
            }
            None => serializer.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for GoFmph {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if let Some(buffer) = <Option<Vec<u8>>>::deserialize(deserializer)? {
            return Ok(Self {
                function: Some(
                    GOFunction::read(&mut std::io::Cursor::new(buffer))
                        .map_err(D::Error::custom)?,
                ),
            })
        }
        Ok(Self { function: None })
    }
}
