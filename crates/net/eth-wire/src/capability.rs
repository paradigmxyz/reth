//! All capability related types

use crate::{
    errors::{P2PHandshakeError, P2PStreamError},
    p2pstream::MAX_RESERVED_MESSAGE_ID,
    protocol::{ProtoVersion, Protocol},
    version::ParseVersionError,
    EthMessage, EthMessageID, EthVersion,
};
use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use derive_more::{Deref, DerefMut};
use reth_codecs::add_arbitrary_tests;
use reth_primitives::bytes::{BufMut, Bytes};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
    fmt,
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
    pub name: Cow<'static, str>,
    /// The version of the subprotocol
    pub version: usize,
}

impl Capability {
    /// Create a new `Capability` with the given name and version.
    pub fn new(name: String, version: usize) -> Self {
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

    /// Returns the [EthVersion::Eth66] capability.
    pub const fn eth_66() -> Self {
        Self::eth(EthVersion::Eth66)
    }

    /// Returns the [EthVersion::Eth67] capability.
    pub const fn eth_67() -> Self {
        Self::eth(EthVersion::Eth67)
    }

    /// Returns the [EthVersion::Eth68] capability.
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
        Capability::eth(value)
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

#[cfg(any(test, feature = "arbitrary"))]
impl proptest::arbitrary::Arbitrary for Capability {
    type Parameters = proptest::arbitrary::ParamsFor<String>;
    fn arbitrary_with(args: Self::Parameters) -> Self::Strategy {
        use proptest::strategy::Strategy;
        proptest::arbitrary::any_with::<String>(args) // TODO: what possible values?
            .prop_flat_map(move |name| {
                proptest::arbitrary::any_with::<usize>(()) // TODO: What's the max?
                    .prop_map(move |version| Capability::new(name.clone(), version))
            })
            .boxed()
    }

    type Strategy = proptest::strategy::BoxedStrategy<Capability>;
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

/// This represents a shared capability, its version, and its message id offset.
///
/// The [offset](SharedCapability::message_id_offset) is the message ID offset for this shared
/// capability, determined during the rlpx handshake.
///
/// See also [Message-id based multiplexing](https://github.com/ethereum/devp2p/blob/master/rlpx.md#message-id-based-multiplexing)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SharedCapability {
    /// The `eth` capability.
    Eth {
        /// (Highest) negotiated version of the eth capability.
        version: EthVersion,
        /// The message ID offset for this capability.
        ///
        /// This represents the message ID offset for the first message of the eth capability in
        /// the message id space.
        offset: u8,
    },
    /// Any other unknown capability.
    UnknownCapability {
        /// Shared capability.
        cap: Capability,
        /// The message ID offset for this capability.
        ///
        /// This represents the message ID offset for the first message of the eth capability in
        /// the message id space.
        offset: u8,
        /// The number of messages of this capability. Needed to calculate range of message IDs in
        /// demuxing.
        messages: u8,
    },
}

impl SharedCapability {
    /// Creates a new [`SharedCapability`] based on the given name, offset, version (and messages
    /// if the capability is custom).
    ///
    /// Returns an error if the offset is equal or less than [`MAX_RESERVED_MESSAGE_ID`].
    pub(crate) fn new(
        name: &str,
        version: u8,
        offset: u8,
        messages: u8,
    ) -> Result<Self, SharedCapabilityError> {
        if offset <= MAX_RESERVED_MESSAGE_ID {
            return Err(SharedCapabilityError::ReservedMessageIdOffset(offset))
        }

        match name {
            "eth" => Ok(Self::eth(EthVersion::try_from(version)?, offset)),
            _ => Ok(Self::UnknownCapability {
                cap: Capability::new(name.to_string(), version as usize),
                offset,
                messages,
            }),
        }
    }

    /// Creates a new [`SharedCapability`] based on the given name, offset, and version.
    pub(crate) const fn eth(version: EthVersion, offset: u8) -> Self {
        Self::Eth { version, offset }
    }

    /// Returns the capability.
    pub fn capability(&self) -> Cow<'_, Capability> {
        match self {
            SharedCapability::Eth { version, .. } => Cow::Owned(Capability::eth(*version)),
            SharedCapability::UnknownCapability { cap, .. } => Cow::Borrowed(cap),
        }
    }

    /// Returns the name of the capability.
    #[inline]
    pub fn name(&self) -> &str {
        match self {
            SharedCapability::Eth { .. } => "eth",
            SharedCapability::UnknownCapability { cap, .. } => cap.name.as_ref(),
        }
    }

    /// Returns true if the capability is eth.
    #[inline]
    pub fn is_eth(&self) -> bool {
        matches!(self, SharedCapability::Eth { .. })
    }

    /// Returns the version of the capability.
    pub fn version(&self) -> u8 {
        match self {
            SharedCapability::Eth { version, .. } => *version as u8,
            SharedCapability::UnknownCapability { cap, .. } => cap.version as u8,
        }
    }

    /// Returns the message ID offset of the current capability.
    ///
    /// This represents the message ID offset for the first message of the eth capability in the
    /// message id space.
    pub fn message_id_offset(&self) -> u8 {
        match self {
            SharedCapability::Eth { offset, .. } => *offset,
            SharedCapability::UnknownCapability { offset, .. } => *offset,
        }
    }

    /// Returns the message ID offset of the current capability relative to the start of the
    /// reserved message id space: [`MAX_RESERVED_MESSAGE_ID`].
    pub fn relative_message_id_offset(&self) -> u8 {
        self.message_id_offset() - MAX_RESERVED_MESSAGE_ID - 1
    }

    /// Returns the number of protocol messages supported by this capability.
    pub fn num_messages(&self) -> u8 {
        match self {
            SharedCapability::Eth { version: _version, .. } => EthMessageID::max() + 1,
            SharedCapability::UnknownCapability { messages, .. } => *messages,
        }
    }
}

/// Non-empty,ordered list of recognized shared capabilities.
///
/// Shared capabilities are ordered alphabetically by case sensitive name.
#[derive(Debug, Clone, Deref, DerefMut, PartialEq, Eq)]
pub struct SharedCapabilities(Vec<SharedCapability>);

impl SharedCapabilities {
    /// Merges the local and peer capabilities and returns a new [`SharedCapabilities`] instance.
    pub fn try_new(
        local_protocols: Vec<Protocol>,
        peer_capabilities: Vec<Capability>,
    ) -> Result<Self, P2PStreamError> {
        Ok(Self(shared_capability_offsets(local_protocols, peer_capabilities)?))
    }

    /// Iterates over the shared capabilities.
    pub fn iter_caps(&self) -> impl Iterator<Item = &SharedCapability> {
        self.0.iter()
    }

    /// Returns the eth capability if it is shared.
    pub fn eth(&self) -> Result<&SharedCapability, P2PStreamError> {
        for cap in self.iter_caps() {
            if cap.is_eth() {
                return Ok(cap)
            }
        }
        Err(P2PStreamError::CapabilityNotShared)
    }

    /// Returns the negotiated eth version if it is shared.
    #[inline]
    pub fn eth_version(&self) -> Result<u8, P2PStreamError> {
        self.eth().map(|cap| cap.version())
    }

    /// Returns true if the shared capabilities contain the given capability.
    #[inline]
    pub fn contains(&self, cap: &Capability) -> bool {
        self.find(cap).is_some()
    }

    /// Returns the shared capability for the given capability.
    #[inline]
    pub fn find(&self, cap: &Capability) -> Option<&SharedCapability> {
        self.0.iter().find(|c| c.version() == cap.version as u8 && c.name() == cap.name)
    }

    /// Returns the matching shared capability for the given capability offset.
    ///
    /// `offset` is the multiplexed message id offset of the capability relative to the reserved
    /// message id space. In other words, counting starts at [`MAX_RESERVED_MESSAGE_ID`] + 1, which
    /// corresponds to the first non-reserved message id.
    ///
    /// For example: `offset == 0` corresponds to the first shared message across the shared
    /// capabilities and will return the first shared capability that supports messages.
    #[inline]
    pub fn find_by_relative_offset(&self, offset: u8) -> Option<&SharedCapability> {
        self.find_by_offset(offset.saturating_add(MAX_RESERVED_MESSAGE_ID + 1))
    }

    /// Returns the matching shared capability for the given capability offset.
    ///
    /// `offset` is the multiplexed message id offset of the capability that includes the reserved
    /// message id space.
    ///
    /// This will always return None if `offset` is less than or equal to
    /// [`MAX_RESERVED_MESSAGE_ID`] because the reserved message id space is not shared.
    #[inline]
    pub fn find_by_offset(&self, offset: u8) -> Option<&SharedCapability> {
        let mut iter = self.0.iter();
        let mut cap = iter.next()?;
        if offset < cap.message_id_offset() {
            // reserved message id space
            return None
        }

        for next in iter {
            if offset < next.message_id_offset() {
                return Some(cap)
            }
            cap = next
        }

        Some(cap)
    }

    /// Returns the shared capability for the given capability or an error if it's not compatible.
    #[inline]
    pub fn ensure_matching_capability(
        &self,
        cap: &Capability,
    ) -> Result<&SharedCapability, UnsupportedCapabilityError> {
        self.find(cap).ok_or_else(|| UnsupportedCapabilityError { capability: cap.clone() })
    }
}

/// Determines the offsets for each shared capability between the input list of peer
/// capabilities and the input list of locally supported [Protocol].
///
/// Additionally, the `p2p` capability version 5 is supported, but is
/// expected _not_ to be in neither `local_protocols` or `peer_capabilities`.
///
/// **Note**: For `local_protocols` this takes [Protocol] because we need to know the number of
/// messages per versioned capability. From the remote we only get the plain [Capability].
#[inline]
pub fn shared_capability_offsets(
    local_protocols: Vec<Protocol>,
    peer_capabilities: Vec<Capability>,
) -> Result<Vec<SharedCapability>, P2PStreamError> {
    // find intersection of capabilities
    let our_capabilities =
        local_protocols.into_iter().map(Protocol::split).collect::<HashMap<_, _>>();

    // map of capability name to version
    let mut shared_capabilities: HashMap<_, ProtoVersion> = HashMap::new();

    // The `Ord` implementation for capability names should be equivalent to geth (and every other
    // client), since geth uses golang's default string comparison, which orders strings
    // lexicographically.
    // https://golang.org/pkg/strings/#Compare
    //
    // This is important because the capability name is used to determine the message id offset, so
    // if the sorting is not identical, offsets for connected peers could be inconsistent.
    // This would cause the peers to send messages with the wrong message id, which is usually a
    // protocol violation.
    //
    // The `Ord` implementation for `str` orders strings lexicographically.
    let mut shared_capability_names = BTreeSet::new();

    // find highest shared version of each shared capability
    for peer_capability in peer_capabilities {
        // if we contain this specific capability both peers share it
        if let Some(messages) = our_capabilities.get(&peer_capability).copied() {
            // If multiple versions are shared of the same (equal name) capability, the numerically
            // highest wins, others are ignored

            let version = shared_capabilities.get(&peer_capability.name).map(|v| v.version);

            // TODO(mattsse): simplify
            if version.is_none() ||
                (version.is_some() && peer_capability.version > version.expect("is some; qed"))
            {
                shared_capabilities.insert(
                    peer_capability.name.clone(),
                    ProtoVersion { version: peer_capability.version, messages },
                );
                shared_capability_names.insert(peer_capability.name);
            }
        }
    }

    // disconnect if we don't share any capabilities
    if shared_capabilities.is_empty() {
        return Err(P2PStreamError::HandshakeError(P2PHandshakeError::NoSharedCapabilities))
    }

    // order versions based on capability name (alphabetical) and select offsets based on
    // BASE_OFFSET + prev_total_message
    let mut shared_with_offsets = Vec::new();

    // Message IDs are assumed to be compact from ID 0x10 onwards (0x00-0x0f is reserved for the
    // "p2p" capability) and given to each shared (equal-version, equal-name) capability in
    // alphabetic order.
    let mut offset = MAX_RESERVED_MESSAGE_ID + 1;
    for name in shared_capability_names {
        let proto_version = shared_capabilities.get(&name).expect("shared; qed");

        let shared_capability = SharedCapability::new(
            &name,
            proto_version.version as u8,
            offset,
            proto_version.messages,
        )?;

        offset += shared_capability.num_messages();
        shared_with_offsets.push(shared_capability);
    }

    if shared_with_offsets.is_empty() {
        return Err(P2PStreamError::HandshakeError(P2PHandshakeError::NoSharedCapabilities))
    }

    Ok(shared_with_offsets)
}

/// An error that may occur while creating a [`SharedCapability`].
#[derive(Debug, thiserror::Error)]
pub enum SharedCapabilityError {
    /// Unsupported `eth` version.
    #[error(transparent)]
    UnsupportedVersion(#[from] ParseVersionError),
    /// Thrown when the message id for a [SharedCapability] overlaps with the reserved p2p message
    /// id space [`MAX_RESERVED_MESSAGE_ID`].
    #[error("message id offset `{0}` is reserved")]
    ReservedMessageIdOffset(u8),
}

/// An error thrown when capabilities mismatch.
#[derive(Debug, thiserror::Error)]
#[error("unsupported capability {capability}")]
pub struct UnsupportedCapabilityError {
    capability: Capability,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_eth_68() {
        let capability = SharedCapability::new("eth", 68, MAX_RESERVED_MESSAGE_ID + 1, 13).unwrap();

        assert_eq!(capability.name(), "eth");
        assert_eq!(capability.version(), 68);
        assert_eq!(
            capability,
            SharedCapability::Eth {
                version: EthVersion::Eth68,
                offset: MAX_RESERVED_MESSAGE_ID + 1
            }
        );
    }

    #[test]
    fn from_eth_67() {
        let capability = SharedCapability::new("eth", 67, MAX_RESERVED_MESSAGE_ID + 1, 13).unwrap();

        assert_eq!(capability.name(), "eth");
        assert_eq!(capability.version(), 67);
        assert_eq!(
            capability,
            SharedCapability::Eth {
                version: EthVersion::Eth67,
                offset: MAX_RESERVED_MESSAGE_ID + 1
            }
        );
    }

    #[test]
    fn from_eth_66() {
        let capability = SharedCapability::new("eth", 66, MAX_RESERVED_MESSAGE_ID + 1, 15).unwrap();

        assert_eq!(capability.name(), "eth");
        assert_eq!(capability.version(), 66);
        assert_eq!(
            capability,
            SharedCapability::Eth {
                version: EthVersion::Eth66,
                offset: MAX_RESERVED_MESSAGE_ID + 1
            }
        );
    }

    #[test]
    fn capabilities_supports_eth() {
        let capabilities: Capabilities = vec![
            Capability::new_static("eth", 66),
            Capability::new_static("eth", 67),
            Capability::new_static("eth", 68),
        ]
        .into();

        assert!(capabilities.supports_eth());
        assert!(capabilities.supports_eth_v66());
        assert!(capabilities.supports_eth_v67());
        assert!(capabilities.supports_eth_v68());
    }

    #[test]
    fn test_peer_lower_capability_version() {
        let local_capabilities: Vec<Protocol> =
            vec![EthVersion::Eth66.into(), EthVersion::Eth67.into(), EthVersion::Eth68.into()];
        let peer_capabilities: Vec<Capability> = vec![EthVersion::Eth66.into()];

        let shared_capability =
            shared_capability_offsets(local_capabilities, peer_capabilities).unwrap()[0].clone();

        assert_eq!(
            shared_capability,
            SharedCapability::Eth {
                version: EthVersion::Eth66,
                offset: MAX_RESERVED_MESSAGE_ID + 1
            }
        )
    }

    #[test]
    fn test_peer_capability_version_too_low() {
        let local: Vec<Protocol> = vec![EthVersion::Eth67.into()];
        let peer_capabilities: Vec<Capability> = vec![EthVersion::Eth66.into()];

        let shared_capability = shared_capability_offsets(local, peer_capabilities);

        assert!(matches!(
            shared_capability,
            Err(P2PStreamError::HandshakeError(P2PHandshakeError::NoSharedCapabilities))
        ))
    }

    #[test]
    fn test_peer_capability_version_too_high() {
        let local_capabilities = vec![EthVersion::Eth66.into()];
        let peer_capabilities = vec![EthVersion::Eth67.into()];

        let shared_capability = shared_capability_offsets(local_capabilities, peer_capabilities);

        assert!(matches!(
            shared_capability,
            Err(P2PStreamError::HandshakeError(P2PHandshakeError::NoSharedCapabilities))
        ))
    }

    #[test]
    fn test_find_by_offset() {
        let local_capabilities = vec![EthVersion::Eth66.into()];
        let peer_capabilities = vec![EthVersion::Eth66.into()];

        let shared = SharedCapabilities::try_new(local_capabilities, peer_capabilities).unwrap();

        let shared_eth = shared.find_by_relative_offset(0).unwrap();
        assert_eq!(shared_eth.name(), "eth");

        let shared_eth = shared.find_by_offset(MAX_RESERVED_MESSAGE_ID + 1).unwrap();
        assert_eq!(shared_eth.name(), "eth");

        // reserved message id space
        assert!(shared.find_by_offset(MAX_RESERVED_MESSAGE_ID).is_none());
    }

    #[test]
    fn test_find_by_offset_many() {
        let cap = Capability::new_static("aaa", 1);
        let proto = Protocol::new(cap.clone(), 5);
        let local_capabilities = vec![proto.clone(), EthVersion::Eth66.into()];
        let peer_capabilities = vec![cap, EthVersion::Eth66.into()];

        let shared = SharedCapabilities::try_new(local_capabilities, peer_capabilities).unwrap();

        let shared_eth = shared.find_by_relative_offset(0).unwrap();
        assert_eq!(shared_eth.name(), proto.cap.name);

        let shared_eth = shared.find_by_offset(MAX_RESERVED_MESSAGE_ID + 1).unwrap();
        assert_eq!(shared_eth.name(), proto.cap.name);

        // the 5th shared message (0,1,2,3,4) is the last message of the aaa capability
        let shared_eth = shared.find_by_relative_offset(4).unwrap();
        assert_eq!(shared_eth.name(), proto.cap.name);
        let shared_eth = shared.find_by_offset(MAX_RESERVED_MESSAGE_ID + 5).unwrap();
        assert_eq!(shared_eth.name(), proto.cap.name);

        // the 6th shared message is the first message of the eth capability
        let shared_eth = shared.find_by_relative_offset(1 + proto.messages()).unwrap();
        assert_eq!(shared_eth.name(), "eth");
    }
}
