use crate::ByteStr;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// JSON-RPC request id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Id {
    /// Null id.
    #[default]
    Null,
    /// Numeric id.
    Number(u64),
    /// String id.
    Str(ByteStr),
}

impl Id {
    /// Returns the numeric id, if any.
    pub const fn as_number(&self) -> Option<u64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Appends the JSON encoding of the id to `buf`.
    pub(crate) fn write_json(&self, buf: &mut Vec<u8>) {
        let _ = serde_json::to_writer(buf, self);
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("null"),
            Self::Number(n) => n.fmt(f),
            Self::Str(s) => s.fmt(f),
        }
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Number(n) => serializer.serialize_u64(*n),
            Self::Str(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Id;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a null, number or string id")
            }

            fn visit_unit<E: de::Error>(self) -> Result<Id, E> {
                Ok(Id::Null)
            }

            fn visit_none<E: de::Error>(self) -> Result<Id, E> {
                Ok(Id::Null)
            }

            fn visit_u64<E: de::Error>(self, n: u64) -> Result<Id, E> {
                Ok(Id::Number(n))
            }

            fn visit_str<E: de::Error>(self, s: &str) -> Result<Id, E> {
                Ok(Id::Str(s.to_owned().into()))
            }

            fn visit_string<E: de::Error>(self, s: String) -> Result<Id, E> {
                Ok(Id::Str(s.into()))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

/// Subscription id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SubscriptionId {
    /// Numeric id.
    Num(u64),
    /// String id.
    Str(String),
}

impl From<u64> for SubscriptionId {
    fn from(n: u64) -> Self {
        Self::Num(n)
    }
}

impl From<String> for SubscriptionId {
    fn from(s: String) -> Self {
        Self::Str(s)
    }
}

impl fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Num(n) => n.fmt(f),
            Self::Str(s) => s.fmt(f),
        }
    }
}
