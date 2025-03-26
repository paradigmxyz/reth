//! Support for representing the version of the `eth`

use crate::alloc::string::ToString;
use alloc::string::String;
use alloy_rlp::{Decodable, Encodable, Error as RlpError};
use bytes::BufMut;
use core::{fmt, str::FromStr};
use derive_more::Display;
use reth_codecs_derive::add_arbitrary_tests;

/// Error thrown when failed to parse a valid [`EthVersion`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Unknown eth protocol version: {0}")]
pub struct ParseVersionError(String);

/// The `eth` protocol version.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Display)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub enum EthVersion {
    /// The `eth` protocol version 66.
    Eth66 = 66,
    /// The `eth` protocol version 67.
    Eth67 = 67,
    /// The `eth` protocol version 68.
    Eth68 = 68,
    /// The `eth` protocol version 69.
    Eth69 = 69,
}

impl EthVersion {
    /// The latest known eth version
    pub const LATEST: Self = Self::Eth68;

    /// Returns the total number of messages the protocol version supports.
    pub const fn total_messages(&self) -> u8 {
        match self {
            Self::Eth66 => 15,
            Self::Eth67 | Self::Eth68 => {
                // eth/67,68 are eth/66 minus GetNodeData and NodeData messages
                13
            }
            // eth69 is both eth67 and eth68 minus NewBlockHashes and NewBlock
            Self::Eth69 => 11,
        }
    }

    /// Returns true if the version is eth/66
    pub const fn is_eth66(&self) -> bool {
        matches!(self, Self::Eth66)
    }

    /// Returns true if the version is eth/67
    pub const fn is_eth67(&self) -> bool {
        matches!(self, Self::Eth67)
    }

    /// Returns true if the version is eth/68
    pub const fn is_eth68(&self) -> bool {
        matches!(self, Self::Eth68)
    }

    /// Returns true if the version is eth/69
    pub const fn is_eth69(&self) -> bool {
        matches!(self, Self::Eth69)
    }
}

/// RLP encodes `EthVersion` as a single byte (66-69).
impl Encodable for EthVersion {
    fn encode(&self, out: &mut dyn BufMut) {
        (*self as u8).encode(out)
    }

    fn length(&self) -> usize {
        (*self as u8).length()
    }
}

/// RLP decodes a single byte into `EthVersion`.
/// Returns error if byte is not a valid version (66-69).
impl Decodable for EthVersion {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let version = u8::decode(buf)?;
        Self::try_from(version).map_err(|_| RlpError::Custom("invalid eth version"))
    }
}

/// Allow for converting from a `&str` to an `EthVersion`.
///
/// # Example
/// ```
/// use reth_eth_wire_types::EthVersion;
///
/// let version = EthVersion::try_from("67").unwrap();
/// assert_eq!(version, EthVersion::Eth67);
/// ```
impl TryFrom<&str> for EthVersion {
    type Error = ParseVersionError;

    #[inline]
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "66" => Ok(Self::Eth66),
            "67" => Ok(Self::Eth67),
            "68" => Ok(Self::Eth68),
            "69" => Ok(Self::Eth69),
            _ => Err(ParseVersionError(s.to_string())),
        }
    }
}

/// Allow for converting from a u8 to an `EthVersion`.
///
/// # Example
/// ```
/// use reth_eth_wire_types::EthVersion;
///
/// let version = EthVersion::try_from(67).unwrap();
/// assert_eq!(version, EthVersion::Eth67);
/// ```
impl TryFrom<u8> for EthVersion {
    type Error = ParseVersionError;

    #[inline]
    fn try_from(u: u8) -> Result<Self, Self::Error> {
        match u {
            66 => Ok(Self::Eth66),
            67 => Ok(Self::Eth67),
            68 => Ok(Self::Eth68),
            69 => Ok(Self::Eth69),
            _ => Err(ParseVersionError(u.to_string())),
        }
    }
}

impl FromStr for EthVersion {
    type Err = ParseVersionError;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl From<EthVersion> for u8 {
    #[inline]
    fn from(v: EthVersion) -> Self {
        v as Self
    }
}

impl From<EthVersion> for &'static str {
    #[inline]
    fn from(v: EthVersion) -> &'static str {
        match v {
            EthVersion::Eth66 => "66",
            EthVersion::Eth67 => "67",
            EthVersion::Eth68 => "68",
            EthVersion::Eth69 => "69",
        }
    }
}

/// RLPx `p2p` protocol version
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub enum ProtocolVersion {
    /// `p2p` version 4
    V4 = 4,
    /// `p2p` version 5
    #[default]
    V5 = 5,
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", *self as u8)
    }
}

impl Encodable for ProtocolVersion {
    fn encode(&self, out: &mut dyn BufMut) {
        (*self as u8).encode(out)
    }
    fn length(&self) -> usize {
        // the version should be a single byte
        (*self as u8).length()
    }
}

impl Decodable for ProtocolVersion {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let version = u8::decode(buf)?;
        match version {
            4 => Ok(Self::V4),
            5 => Ok(Self::V5),
            _ => Err(RlpError::Custom("unknown p2p protocol version")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EthVersion, ParseVersionError};
    use alloy_rlp::{Decodable, Encodable, Error as RlpError};
    use bytes::BytesMut;

    #[test]
    fn test_eth_version_try_from_str() {
        assert_eq!(EthVersion::Eth66, EthVersion::try_from("66").unwrap());
        assert_eq!(EthVersion::Eth67, EthVersion::try_from("67").unwrap());
        assert_eq!(EthVersion::Eth68, EthVersion::try_from("68").unwrap());
        assert_eq!(EthVersion::Eth69, EthVersion::try_from("69").unwrap());
        assert_eq!(Err(ParseVersionError("70".to_string())), EthVersion::try_from("70"));
    }

    #[test]
    fn test_eth_version_from_str() {
        assert_eq!(EthVersion::Eth66, "66".parse().unwrap());
        assert_eq!(EthVersion::Eth67, "67".parse().unwrap());
        assert_eq!(EthVersion::Eth68, "68".parse().unwrap());
        assert_eq!(EthVersion::Eth69, "69".parse().unwrap());
        assert_eq!(Err(ParseVersionError("70".to_string())), "70".parse::<EthVersion>());
    }

    #[test]
    fn test_eth_version_rlp_encode() {
        let versions = [EthVersion::Eth66, EthVersion::Eth67, EthVersion::Eth68, EthVersion::Eth69];

        for version in versions {
            let mut encoded = BytesMut::new();
            version.encode(&mut encoded);

            assert_eq!(encoded.len(), 1);
            assert_eq!(encoded[0], version as u8);
        }
    }
    #[test]
    fn test_eth_version_rlp_decode() {
        let test_cases = [
            (66_u8, Ok(EthVersion::Eth66)),
            (67_u8, Ok(EthVersion::Eth67)),
            (68_u8, Ok(EthVersion::Eth68)),
            (69_u8, Ok(EthVersion::Eth69)),
            (70_u8, Err(RlpError::Custom("invalid eth version"))),
            (65_u8, Err(RlpError::Custom("invalid eth version"))),
        ];

        for (input, expected) in test_cases {
            let mut encoded = BytesMut::new();
            input.encode(&mut encoded);

            let mut slice = encoded.as_ref();
            let result = EthVersion::decode(&mut slice);
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn test_eth_version_total_messages() {
        assert_eq!(EthVersion::Eth66.total_messages(), 15);
        assert_eq!(EthVersion::Eth67.total_messages(), 13);
        assert_eq!(EthVersion::Eth68.total_messages(), 13);
        assert_eq!(EthVersion::Eth69.total_messages(), 11);
    }
}
