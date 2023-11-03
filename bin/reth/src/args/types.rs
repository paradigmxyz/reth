//! Additional helper types for CLI parsing.

use std::{fmt, str::FromStr};

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
