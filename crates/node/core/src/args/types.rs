//! Additional helper types for CLI parsing.

use std::{fmt, num::ParseIntError, str::FromStr};

/// A macro that generates types that maps "0" to "None" when parsing CLI arguments.
macro_rules! zero_as_none {
    ($type_name:ident, $inner_type:ty) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        /// A helper type that maps `0` to `None` when parsing CLI arguments.
        pub struct $type_name(pub Option<$inner_type>);

        impl $type_name {
            /// Returns the inner value.
            pub const fn new(value: $inner_type) -> Self {
                Self(Some(value))
            }

            /// Returns the inner value or `$inner_type::MAX` if `None`.
            pub fn unwrap_or_max(self) -> $inner_type {
                self.0.unwrap_or(<$inner_type>::MAX)
            }
        }

        impl std::fmt::Display for $type_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self.0 {
                    Some(value) => write!(f, "{}", value),
                    None => write!(f, "0"),
                }
            }
        }

        impl From<$inner_type> for $type_name {
            #[inline]
            fn from(value: $inner_type) -> Self {
                Self(if value == 0 { None } else { Some(value) })
            }
        }

        impl std::str::FromStr for $type_name {
            type Err = std::num::ParseIntError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let value = s.parse::<$inner_type>()?;
                Ok(Self::from(value))
            }
        }
    };
}

zero_as_none!(ZeroAsNoneU64, u64);
zero_as_none!(ZeroAsNoneU32, u32);

/// A macro that generates types that map "max" to "MAX" when parsing CLI arguments.
macro_rules! max_values {
    ($name:ident, $ty:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        /// A helper type for parsing "max" as the maximum value of the specified type.

        pub struct $name(pub $ty);

        impl $name {
            /// Returns the inner value.
            pub const fn get(&self) -> $ty {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<$ty> for $name {
            #[inline]
            fn from(value: $ty) -> Self {
                Self(value)
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
max_values!(MaxU32, u32);
max_values!(MaxU64, u64);

/// A helper type that supports parsing "max" or delegates to another parser
#[derive(Debug, Clone)]
pub struct MaxOr<T> {
    /// The inner parser
    inner: T,
    /// The parsed value
    value: u64,
}

impl<T> MaxOr<T>
where
    T: clap::builder::TypedValueParser,
    T::Value: Into<u64>,
{
    /// Creates a new instance with the given inner parser
    pub fn new(inner: T) -> Self {
        Self { inner, value: 0 }
    }

    /// Returns the parsed value
    pub fn get(&self) -> u64 {
        self.value
    }
}

impl<T> clap::builder::TypedValueParser for MaxOr<T>
where
    T: clap::builder::TypedValueParser,
    T::Value: Into<u64>,
{
    type Value = u64;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, clap::Error> {
        if value.to_str().map(|s| s.eq_ignore_ascii_case("max")).unwrap_or(false) {
            Ok(u64::MAX)
        } else {
            self.inner.parse_ref(cmd, arg, value).map(Into::into)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_parse() {
        let val = "0".parse::<ZeroAsNoneU64>().unwrap();
        assert_eq!(val, ZeroAsNoneU64(None));
        assert_eq!(val.unwrap_or_max(), u64::MAX);
    }

    #[test]
    fn test_from_u64() {
        let original = 1u64;
        let expected = ZeroAsNoneU64(Some(1u64));
        assert_eq!(ZeroAsNoneU64::from(original), expected);

        let original = 0u64;
        let expected = ZeroAsNoneU64(None);
        assert_eq!(ZeroAsNoneU64::from(original), expected);
    }
}
