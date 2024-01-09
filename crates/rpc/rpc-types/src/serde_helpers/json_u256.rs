//! Json U256 serde helpers.

use alloy_primitives::U256;
use serde::{
    de::{Error, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use serde_json::Value;
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

impl fmt::Display for JsonU256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
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

/// Visitor pattern for `JsonU256` deserialization.
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
        let value = u256_from_str(value)?;
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

/// Supports deserializing a [U256] from a [String].
pub fn u256_from_str<E>(raw: &str) -> Result<U256, E>
where
    E: Error,
{
    let value = match raw.len() {
        0 => U256::ZERO,
        2 if raw.starts_with("0x") => U256::ZERO,
        _ if raw.starts_with("0x") => U256::from_str(raw)
            .map_err(|e| Error::custom(format!("Parsing JsonU256 as hex failed {raw}: {e}")))?,
        _ => U256::from_str_radix(raw, 10).map_err(|e| {
            Error::custom(format!("Parsing JsonU256 as decimal failed {raw}: {e:?}"))
        })?,
    };

    Ok(value)
}

/// Supports parsing the TTD as an `Option<u64>`, or `Option<f64>` specifically for the mainnet TTD
/// (5.875e22).
pub fn deserialize_json_ttd_opt<'de, D>(deserializer: D) -> Result<Option<U256>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    value.map(|value| ttd_from_value::<'de, D>(value)).transpose()
}

/// Converts the given [serde_json::Value] into a `U256` value for TTD deserialization.
fn ttd_from_value<'de, D>(val: Value) -> Result<U256, D::Error>
where
    D: Deserializer<'de>,
{
    let val = match val {
        Value::Number(num) => num,
        Value::String(raw) => return u256_from_str(&raw),
        _ => return Err(Error::custom("TTD must be a number or string")),
    };

    let num = if let Some(val) = val.as_u64() {
        U256::from(val)
    } else if let Some(value) = val.as_f64() {
        // The ethereum mainnet TTD is 58750000000000000000000, and geth serializes this
        // without quotes, because that is how golang `big.Int`s marshal in JSON. Numbers
        // are arbitrary precision in JSON, so this is valid JSON. This number is also
        // greater than a `u64`.
        //
        // Unfortunately, serde_json only supports parsing up to `u64`, resorting to `f64`
        // once `u64` overflows:
        // <https://github.com/serde-rs/json/blob/4bc1eaa03a6160593575bc9bc60c94dba4cab1e3/src/de.rs#L1411-L1415>
        // <https://github.com/serde-rs/json/blob/4bc1eaa03a6160593575bc9bc60c94dba4cab1e3/src/de.rs#L479-L484>
        // <https://github.com/serde-rs/json/blob/4bc1eaa03a6160593575bc9bc60c94dba4cab1e3/src/de.rs#L102-L108>
        //
        // serde_json does have an arbitrary precision feature, but this breaks untagged
        // enums in serde:
        // <https://github.com/serde-rs/serde/issues/2230>
        // <https://github.com/serde-rs/serde/issues/1183>
        //
        // To solve this, we use the captured float and return the TTD as a U256 if it's equal.
        if value == 5.875e22 {
            U256::from(58750000000000000000000u128)
        } else {
            // We could try to convert to a u128 here but there would probably be loss of
            // precision, so we just return an error.
            return Err(Error::custom("Deserializing a large non-mainnet TTD is not supported"))
        }
    } else {
        // must be i64 - negative numbers are not supported
        return Err(Error::custom("Negative TTD values are invalid and will not be deserialized"))
    };

    Ok(num)
}

#[cfg(test)]
mod test {
    use super::JsonU256;
    use alloy_primitives::U256;
    use serde::{Deserialize, Serialize};

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

    #[test]
    fn deserialize_ttd() {
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
        struct Ttd(#[serde(deserialize_with = "super::deserialize_json_ttd_opt")] Option<U256>);

        let deserialized: Vec<Ttd> = serde_json::from_str(
            r#"["",0,"0","0x0","58750000000000000000000",58750000000000000000000]"#,
        )
        .unwrap();
        assert_eq!(
            deserialized,
            vec![
                Ttd(Some(U256::ZERO)),
                Ttd(Some(U256::ZERO)),
                Ttd(Some(U256::ZERO)),
                Ttd(Some(U256::ZERO)),
                Ttd(Some(U256::from(58750000000000000000000u128))),
                Ttd(Some(U256::from(58750000000000000000000u128))),
            ]
        );
    }
}
