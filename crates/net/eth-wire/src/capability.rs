//! All capability related types

use crate::{version::ParseVersionError, EthMessage, EthVersion};
use reth_codecs::add_arbitrary_tests;
use reth_primitives::bytes::{BufMut, Bytes};
use reth_rlp::{Decodable, DecodeError, Encodable, RlpDecodable, RlpEncodable};
use smol_str::SmolStr;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[cfg(any(test, feature = "arbitrary"))]
use proptest::{
    arbitrary::{any_with, ParamsFor},
    strategy::{BoxedStrategy, Strategy},
};

/// A Capability message consisting of the message-id and the payload
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RawCapabilityMessage {
    /// Identifier of the message.
    pub id: usize,
    /// Actual payload
    pub payload: Bytes,
}

/// Various protocol related event types bubbled up from a session that need to be handled by the
/// network.
#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum CapabilityMessage {
    /// Eth sub-protocol message.
    Eth(EthMessage),
    /// Any other capability message.
    Other(RawCapabilityMessage),
}

/// A message indicating a supported capability and capability version.
#[add_arbitrary_tests(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Capability {
    /// The name of the subprotocol
    pub name: SmolStr,
    /// The version of the subprotocol
    pub version: usize,
}

impl Capability {
    /// Create a new `Capability` with the given name and version.
    pub fn new(name: SmolStr, version: usize) -> Self {
        Self { name, version }
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
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for Capability {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let version = u.int_in_range(0..=32)?; // TODO: What's the max?
        let name: SmolStr = String::arbitrary(u)?.into(); // TODO: what possible values?
        Ok(Self { name, version })
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl proptest::arbitrary::Arbitrary for Capability {
    type Parameters = ParamsFor<String>;
    type Strategy = BoxedStrategy<Capability>;

    fn arbitrary_with(args: Self::Parameters) -> Self::Strategy {
        any_with::<String>(args) // TODO: what possible values?
            .prop_flat_map(move |name| {
                any_with::<usize>(()) // TODO: What's the max?
                    .prop_map(move |version| Capability { name: name.clone().into(), version })
            })
            .boxed()
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
    pub fn supports_eth(&self) -> bool {
        self.eth_68 || self.eth_67 || self.eth_66
    }

    /// Whether this peer supports eth v66 protocol.
    #[inline]
    pub fn supports_eth_v66(&self) -> bool {
        self.eth_66
    }

    /// Whether this peer supports eth v67 protocol.
    #[inline]
    pub fn supports_eth_v67(&self) -> bool {
        self.eth_67
    }

    /// Whether this peer supports eth v68 protocol.
    #[inline]
    pub fn supports_eth_v68(&self) -> bool {
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
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let inner = Vec::<Capability>::decode(buf)?;

        Ok(Self {
            eth_66: inner.iter().any(Capability::is_eth_v66),
            eth_67: inner.iter().any(Capability::is_eth_v67),
            eth_68: inner.iter().any(Capability::is_eth_v68),
            inner,
        })
    }
}

/// This represents a shared capability, its version, and its offset.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum SharedCapability {
    /// The `eth` capability.
    Eth { version: EthVersion, offset: u8 },

    /// An unknown capability.
    UnknownCapability { name: SmolStr, version: u8, offset: u8 },
}

impl SharedCapability {
    /// Creates a new [`SharedCapability`] based on the given name, offset, and version.
    pub(crate) fn new(name: &str, version: u8, offset: u8) -> Result<Self, SharedCapabilityError> {
        match name {
            "eth" => Ok(Self::Eth { version: EthVersion::try_from(version)?, offset }),
            _ => Ok(Self::UnknownCapability { name: name.into(), version, offset }),
        }
    }

    /// Returns the name of the capability.
    pub fn name(&self) -> &str {
        match self {
            SharedCapability::Eth { .. } => "eth",
            SharedCapability::UnknownCapability { name, .. } => name,
        }
    }

    /// Returns the version of the capability.
    pub fn version(&self) -> u8 {
        match self {
            SharedCapability::Eth { version, .. } => *version as u8,
            SharedCapability::UnknownCapability { version, .. } => *version,
        }
    }

    /// Returns the message ID offset of the current capability.
    pub fn offset(&self) -> u8 {
        match self {
            SharedCapability::Eth { offset, .. } => *offset,
            SharedCapability::UnknownCapability { offset, .. } => *offset,
        }
    }

    /// Returns the number of protocol messages supported by this capability.
    pub fn num_messages(&self) -> Result<u8, SharedCapabilityError> {
        match self {
            SharedCapability::Eth { version, .. } => Ok(version.total_messages()),
            _ => Err(SharedCapabilityError::UnknownCapability),
        }
    }
}

/// An error that may occur while creating a [`SharedCapability`].
#[derive(Debug, thiserror::Error)]
pub enum SharedCapabilityError {
    /// Unsupported `eth` version.
    #[error(transparent)]
    UnsupportedVersion(#[from] ParseVersionError),
    /// Cannot determine the number of messages for unknown capabilities.
    #[error("cannot determine the number of messages for unknown capabilities")]
    UnknownCapability,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_eth_68() {
        let capability = SharedCapability::new("eth", 68, 0).unwrap();

        assert_eq!(capability.name(), "eth");
        assert_eq!(capability.version(), 68);
        assert_eq!(capability, SharedCapability::Eth { version: EthVersion::Eth68, offset: 0 });
    }

    #[test]
    fn from_eth_67() {
        let capability = SharedCapability::new("eth", 67, 0).unwrap();

        assert_eq!(capability.name(), "eth");
        assert_eq!(capability.version(), 67);
        assert_eq!(capability, SharedCapability::Eth { version: EthVersion::Eth67, offset: 0 });
    }

    #[test]
    fn from_eth_66() {
        let capability = SharedCapability::new("eth", 66, 0).unwrap();

        assert_eq!(capability.name(), "eth");
        assert_eq!(capability.version(), 66);
        assert_eq!(capability, SharedCapability::Eth { version: EthVersion::Eth66, offset: 0 });
    }

    #[test]
    fn capabilities_supports_eth() {
        let capabilities: Capabilities = vec![
            Capability::new("eth".into(), 66),
            Capability::new("eth".into(), 67),
            Capability::new("eth".into(), 68),
        ]
        .into();

        assert!(capabilities.supports_eth());
        assert!(capabilities.supports_eth_v66());
        assert!(capabilities.supports_eth_v67());
        assert!(capabilities.supports_eth_v68());
    }
}
