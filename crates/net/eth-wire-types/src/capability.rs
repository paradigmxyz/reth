//! All capability related types

use crate::EthVersion;
use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use bytes::BufMut;
use reth_codecs_derive::add_arbitrary_tests;
use std::{borrow::Cow, fmt};

/// A message indicating a supported capability and capability version.
#[add_arbitrary_tests(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Capability {
    /// The name of the subprotocol
    pub name: Cow<'static, str>,
    /// The version of the subprotocol
    pub version: usize,
}

impl Capability {
    /// Create a new `Capability` with the given name and version.
    pub const fn new(name: String, version: usize) -> Self {
        Self { name: Cow::Owned(name), version }
    }

    /// Create a new `Capability` with the given static name and version.
    pub const fn new_static(name: &'static str, version: usize) -> Self {
        Self { name: Cow::Borrowed(name), version }
    }

    /// Returns the corresponding eth capability for the given version.
    pub const fn eth(version: EthVersion) -> Self {
        Self::new_static("eth", version as usize)
    }

    /// Returns the [`EthVersion::Eth66`] capability.
    pub const fn eth_66() -> Self {
        Self::eth(EthVersion::Eth66)
    }

    /// Returns the [`EthVersion::Eth67`] capability.
    pub const fn eth_67() -> Self {
        Self::eth(EthVersion::Eth67)
    }

    /// Returns the [`EthVersion::Eth68`] capability.
    pub const fn eth_68() -> Self {
        Self::eth(EthVersion::Eth68)
    }

    /// Whether this is eth v66 protocol.
    #[inline]
    pub fn is_eth_v66(&self) -> bool {
        self.name == "eth" && self.version == 66
    }

    /// Whether this is eth v67.
    #[inline]
    pub fn is_eth_v67(&self) -> bool {
        self.name == "eth" && self.version == 67
    }

    /// Whether this is eth v68.
    #[inline]
    pub fn is_eth_v68(&self) -> bool {
        self.name == "eth" && self.version == 68
    }

    /// Whether this is any eth version.
    #[inline]
    pub fn is_eth(&self) -> bool {
        self.is_eth_v66() || self.is_eth_v67() || self.is_eth_v68()
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.name, self.version)
    }
}

impl From<EthVersion> for Capability {
    #[inline]
    fn from(value: EthVersion) -> Self {
        Self::eth(value)
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for Capability {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let version = u.int_in_range(0..=32)?; // TODO: What's the max?
        let name = String::arbitrary(u)?; // TODO: what possible values?
        Ok(Self::new(name, version))
    }
}

/// Represents all capabilities of a node.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Capabilities {
    /// All Capabilities and their versions
    inner: Vec<Capability>,
    eth_66: bool,
    eth_67: bool,
    eth_68: bool,
}

impl Capabilities {
    /// Returns all capabilities.
    #[inline]
    pub fn capabilities(&self) -> &[Capability] {
        &self.inner
    }

    /// Consumes the type and returns the all capabilities.
    #[inline]
    pub fn into_inner(self) -> Vec<Capability> {
        self.inner
    }

    /// Whether the peer supports `eth` sub-protocol.
    #[inline]
    pub const fn supports_eth(&self) -> bool {
        self.eth_68 || self.eth_67 || self.eth_66
    }

    /// Whether this peer supports eth v66 protocol.
    #[inline]
    pub const fn supports_eth_v66(&self) -> bool {
        self.eth_66
    }

    /// Whether this peer supports eth v67 protocol.
    #[inline]
    pub const fn supports_eth_v67(&self) -> bool {
        self.eth_67
    }

    /// Whether this peer supports eth v68 protocol.
    #[inline]
    pub const fn supports_eth_v68(&self) -> bool {
        self.eth_68
    }
}

impl From<Vec<Capability>> for Capabilities {
    fn from(value: Vec<Capability>) -> Self {
        Self {
            eth_66: value.iter().any(Capability::is_eth_v66),
            eth_67: value.iter().any(Capability::is_eth_v67),
            eth_68: value.iter().any(Capability::is_eth_v68),
            inner: value,
        }
    }
}

impl Encodable for Capabilities {
    fn encode(&self, out: &mut dyn BufMut) {
        self.inner.encode(out)
    }
}

impl Decodable for Capabilities {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let inner = Vec::<Capability>::decode(buf)?;

        Ok(Self {
            eth_66: inner.iter().any(Capability::is_eth_v66),
            eth_67: inner.iter().any(Capability::is_eth_v67),
            eth_68: inner.iter().any(Capability::is_eth_v68),
            inner,
        })
    }
}
