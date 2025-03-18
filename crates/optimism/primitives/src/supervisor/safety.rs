//! Message safety level for interoperability.
// Source: https://github.com/op-rs/kona
// Copyright © 2023 kona contributors Copyright © 2024 Optimism
//
// Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
// associated documentation files (the “Software”), to deal in the Software without restriction,
// including without limitation the rights to use, copy, modify, merge, publish, distribute,
// sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all copies or
// substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
// NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
// NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
// DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
use alloc::string::{String, ToString};
use core::str::FromStr;
use derive_more::Display;
use thiserror::Error;
/// The safety level of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SafetyLevel {
    /// The message is finalized.
    Finalized,
    /// The message is safe.
    Safe,
    /// The message is safe locally.
    LocalSafe,
    /// The message is unsafe across chains.
    CrossUnsafe,
    /// The message is unsafe.
    Unsafe,
    /// The message is invalid.
    Invalid,
}

impl FromStr for SafetyLevel {
    type Err = SafetyLevelParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "finalized" => Ok(Self::Finalized),
            "safe" => Ok(Self::Safe),
            "local-safe" | "localsafe" => Ok(Self::LocalSafe),
            "cross-unsafe" | "crossunsafe" => Ok(Self::CrossUnsafe),
            "unsafe" => Ok(Self::Unsafe),
            "invalid" => Ok(Self::Invalid),
            _ => Err(SafetyLevelParseError(s.to_string())),
        }
    }
}

/// Error when parsing [`SafetyLevel`] from string.
#[derive(Error, Debug)]
#[error("Invalid SafetyLevel, error: {0}")]
pub struct SafetyLevelParseError(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "serde")]
    fn test_safety_level_serde() {
        let level = SafetyLevel::Finalized;
        let json = serde_json::to_string(&level).unwrap();
        assert_eq!(json, r#""finalized""#);

        let level: SafetyLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(level, SafetyLevel::Finalized);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_serde_safety_level_fails() {
        let json = r#""failed""#;
        let level: Result<SafetyLevel, _> = serde_json::from_str(json);
        assert!(level.is_err());
    }

    #[test]
    fn test_safety_level_from_str_valid() {
        assert_eq!(SafetyLevel::from_str("finalized").unwrap(), SafetyLevel::Finalized);
        assert_eq!(SafetyLevel::from_str("safe").unwrap(), SafetyLevel::Safe);
        assert_eq!(SafetyLevel::from_str("local-safe").unwrap(), SafetyLevel::LocalSafe);
        assert_eq!(SafetyLevel::from_str("localsafe").unwrap(), SafetyLevel::LocalSafe);
        assert_eq!(SafetyLevel::from_str("cross-unsafe").unwrap(), SafetyLevel::CrossUnsafe);
        assert_eq!(SafetyLevel::from_str("crossunsafe").unwrap(), SafetyLevel::CrossUnsafe);
        assert_eq!(SafetyLevel::from_str("unsafe").unwrap(), SafetyLevel::Unsafe);
        assert_eq!(SafetyLevel::from_str("invalid").unwrap(), SafetyLevel::Invalid);
    }

    #[test]
    fn test_safety_level_from_str_invalid() {
        assert!(SafetyLevel::from_str("unknown").is_err());
        assert!(SafetyLevel::from_str("123").is_err());
        assert!(SafetyLevel::from_str("").is_err());
        assert!(SafetyLevel::from_str("safe ").is_err());
    }
}
