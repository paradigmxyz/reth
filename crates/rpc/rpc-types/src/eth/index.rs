use alloy_primitives::U256;
use serde::{
    de::{Error, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::fmt;

/// A hex encoded or decimal index that's intended to be used as a rust index, hence it's
/// deserialized into a `usize`.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy, Default)]
pub struct Index(usize);

impl From<Index> for usize {
    fn from(idx: Index) -> Self {
        idx.0
    }
}

impl From<Index> for U256 {
    fn from(idx: Index) -> Self {
        U256::from(idx.0)
    }
}

impl From<usize> for Index {
    fn from(idx: usize) -> Self {
        Index(idx)
    }
}

impl Serialize for Index {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("0x{:x}", self.0))
    }
}

impl<'a> Deserialize<'a> for Index {
    fn deserialize<D>(deserializer: D) -> Result<Index, D::Error>
    where
        D: Deserializer<'a>,
    {
        struct IndexVisitor;

        impl<'a> Visitor<'a> for IndexVisitor {
            type Value = Index;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "hex-encoded or decimal index")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: Error,
            {
                Ok(Index(value as usize))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                if let Some(val) = value.strip_prefix("0x") {
                    usize::from_str_radix(val, 16).map(Index).map_err(|e| {
                        Error::custom(format!("Failed to parse hex encoded index value: {e}"))
                    })
                } else {
                    value
                        .parse::<usize>()
                        .map(Index)
                        .map_err(|e| Error::custom(format!("Failed to parse numeric index: {e}")))
                }
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: Error,
            {
                self.visit_str(value.as_ref())
            }
        }

        deserializer.deserialize_any(IndexVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{thread_rng, Rng};

    #[test]
    fn test_serde_index_rand() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let index = Index(rng.gen());
            let val = serde_json::to_string(&index).unwrap();
            let de: Index = serde_json::from_str(&val).unwrap();
            assert_eq!(index, de);
        }
    }
}
