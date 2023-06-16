use crate::U256;
use serde::{
    de::{Error, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::{fmt, str::FromStr};

/// Wrapper around primitive U256 type that also supports deserializing numbers
#[derive(Default, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct JsonU256(pub U256);

impl From<JsonU256> for U256 {
    fn from(value: JsonU256) -> Self {
        value.0
    }
}

impl From<U256> for JsonU256 {
    fn from(value: U256) -> Self {
        JsonU256(value)
    }
}

impl Serialize for JsonU256 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'a> Deserialize<'a> for JsonU256 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'a>,
    {
        deserializer.deserialize_any(JsonU256Visitor)
    }
}

struct JsonU256Visitor;

impl<'a> Visitor<'a> for JsonU256Visitor {
    type Value = JsonU256;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "a hex encoding or decimal number")
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: Error,
    {
        Ok(JsonU256(U256::from(value)))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: Error,
    {
        let value = match value.len() {
            0 => U256::ZERO,
            2 if value.starts_with("0x") => U256::ZERO,
            _ if value.starts_with("0x") => U256::from_str(value).map_err(|e| {
                Error::custom(format!("Parsing JsonU256 as hex failed {value}: {e}"))
            })?,
            _ => U256::from_str_radix(value, 10).map_err(|e| {
                Error::custom(format!("Parsing JsonU256 as decimal failed {value}: {e:?}"))
            })?,
        };

        Ok(JsonU256(value))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: Error,
    {
        self.visit_str(value.as_ref())
    }
}

/// Supports parsing `U256` numbers as strings via [JsonU256]
pub fn deserialize_json_u256<'de, D>(deserializer: D) -> Result<U256, D::Error>
where
    D: Deserializer<'de>,
{
    let num = JsonU256::deserialize(deserializer)?;
    Ok(num.into())
}

/// Supports parsing `U256` numbers as strings via [JsonU256]
pub fn deserialize_json_u256_opt<'de, D>(deserializer: D) -> Result<Option<U256>, D::Error>
where
    D: Deserializer<'de>,
{
    let num = Option::<JsonU256>::deserialize(deserializer)?;
    Ok(num.map(Into::into))
}

#[cfg(test)]
mod test {
    use super::JsonU256;
    use crate::U256;

    #[test]
    fn jsonu256_deserialize() {
        let deserialized: Vec<JsonU256> =
            serde_json::from_str(r#"["","0", "0x","10",10,"0x10"]"#).unwrap();
        assert_eq!(
            deserialized,
            vec![
                JsonU256(U256::ZERO),
                JsonU256(U256::ZERO),
                JsonU256(U256::ZERO),
                JsonU256(U256::from(10)),
                JsonU256(U256::from(10)),
                JsonU256(U256::from(16)),
            ]
        );
    }

    #[test]
    fn jsonu256_serialize() {
        let data = JsonU256(U256::from(16));
        let serialized = serde_json::to_string(&data).unwrap();

        assert_eq!(serialized, r#""0x10""#);
    }
}
