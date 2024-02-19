//! Support for representing the version of the `eth`. [`Capability`](crate::capability::Capability)
//! and [Protocol](crate::protocol::Protocol).

use std::str::FromStr;

use derive_more::Display;

/// Error thrown when failed to parse a valid [`EthVersion`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Unknown eth protocol version: {0}")]
pub struct ParseVersionError(String);

/// The `eth` protocol version.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Display)]
pub enum EthVersion {
    /// The `eth` protocol version 66.
    Eth66 = 66,

    /// The `eth` protocol version 67.
    Eth67 = 67,

    /// The `eth` protocol version 68.
    Eth68 = 68,
}

impl EthVersion {
    /// The latest known eth version
    pub const LATEST: EthVersion = EthVersion::Eth68;

    /// Returns the total number of messages the protocol version supports.
    pub const fn total_messages(&self) -> u8 {
        match self {
            EthVersion::Eth66 => 15,
            EthVersion::Eth67 | EthVersion::Eth68 => {
                // eth/67,68 are eth/66 minus GetNodeData and NodeData messages
                13
            }
        }
    }

    /// Returns true if the version is eth/66
    pub const fn is_eth66(&self) -> bool {
        matches!(self, EthVersion::Eth66)
    }

    /// Returns true if the version is eth/67
    pub const fn is_eth67(&self) -> bool {
        matches!(self, EthVersion::Eth67)
    }

    /// Returns true if the version is eth/68
    pub const fn is_eth68(&self) -> bool {
        matches!(self, EthVersion::Eth68)
    }
}

/// Allow for converting from a `&str` to an `EthVersion`.
///
/// # Example
/// ```
/// use reth_eth_wire::types::EthVersion;
///
/// let version = EthVersion::try_from("67").unwrap();
/// assert_eq!(version, EthVersion::Eth67);
/// ```
impl TryFrom<&str> for EthVersion {
    type Error = ParseVersionError;

    #[inline]
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "66" => Ok(EthVersion::Eth66),
            "67" => Ok(EthVersion::Eth67),
            "68" => Ok(EthVersion::Eth68),
            _ => Err(ParseVersionError(s.to_string())),
        }
    }
}

/// Allow for converting from a u8 to an `EthVersion`.
///
/// # Example
/// ```
/// use reth_eth_wire::types::EthVersion;
///
/// let version = EthVersion::try_from(67).unwrap();
/// assert_eq!(version, EthVersion::Eth67);
/// ```
impl TryFrom<u8> for EthVersion {
    type Error = ParseVersionError;

    #[inline]
    fn try_from(u: u8) -> Result<Self, Self::Error> {
        match u {
            66 => Ok(EthVersion::Eth66),
            67 => Ok(EthVersion::Eth67),
            68 => Ok(EthVersion::Eth68),
            _ => Err(ParseVersionError(u.to_string())),
        }
    }
}

impl FromStr for EthVersion {
    type Err = ParseVersionError;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        EthVersion::try_from(s)
    }
}

impl From<EthVersion> for u8 {
    #[inline]
    fn from(v: EthVersion) -> u8 {
        v as u8
    }
}

impl From<EthVersion> for &'static str {
    #[inline]
    fn from(v: EthVersion) -> &'static str {
        match v {
            EthVersion::Eth66 => "66",
            EthVersion::Eth67 => "67",
            EthVersion::Eth68 => "68",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EthVersion, ParseVersionError};

    #[test]
    fn test_eth_version_try_from_str() {
        assert_eq!(EthVersion::Eth66, EthVersion::try_from("66").unwrap());
        assert_eq!(EthVersion::Eth67, EthVersion::try_from("67").unwrap());
        assert_eq!(EthVersion::Eth68, EthVersion::try_from("68").unwrap());
        assert_eq!(Err(ParseVersionError("69".to_string())), EthVersion::try_from("69"));
    }

    #[test]
    fn test_eth_version_from_str() {
        assert_eq!(EthVersion::Eth66, "66".parse().unwrap());
        assert_eq!(EthVersion::Eth67, "67".parse().unwrap());
        assert_eq!(EthVersion::Eth68, "68".parse().unwrap());
        assert_eq!(Err(ParseVersionError("69".to_string())), "69".parse::<EthVersion>());
    }
}
