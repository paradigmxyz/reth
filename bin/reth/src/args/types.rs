//! Additional helper types for CLI parsing.

use std::{fmt, num::ParseIntError, str::FromStr};

/// A helper type that maps `0` to `None` when parsing CLI arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZeroAsNone(pub Option<u64>);

impl ZeroAsNone {
    /// Returns the inner value.
    pub const fn new(value: u64) -> Self {
        Self(Some(value))
    }

    /// Returns the inner value or `u64::MAX` if `None`.
    pub fn unwrap_or_max(self) -> u64 {
        self.0.unwrap_or(u64::MAX)
    }
}

impl fmt::Display for ZeroAsNone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(value) => write!(f, "{}", value),
            None => write!(f, "0"),
        }
    }
}

impl FromStr for ZeroAsNone {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let value = s.parse::<u64>()?;
        Ok(Self(if value == 0 { None } else { Some(value) }))
    }
}

macro_rules! max_values {
    ($name:ident, $ty:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        /// A helper type for parsing "max" as the maximum value of the specified type.

        pub(crate) struct $name(pub(crate) $ty);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = ParseIntError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                if s.eq_ignore_ascii_case("max") {
                    Ok($name(<$ty>::MAX))
                } else {
                    s.parse::<$ty>().map($name)
                }
            }
        }
    };
}
max_values!(U32, u32);
max_values!(U64, u64);
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_parse() {
        let val = "0".parse::<ZeroAsNone>().unwrap();
        assert_eq!(val, ZeroAsNone(None));
        assert_eq!(val.unwrap_or_max(), u64::MAX);
    }
}
