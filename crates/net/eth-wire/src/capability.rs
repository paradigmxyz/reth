//! All capability related types

use crate::{
    errors::{P2PHandshakeError, P2PStreamError},
    p2pstream::MAX_RESERVED_MESSAGE_ID,
    protocol::{ProtoVersion, Protocol},
    version::ParseVersionError,
    Capability, EthMessageID, EthVersion,
};
use alloy_primitives::bytes::Bytes;
use derive_more::{Deref, DerefMut};
use reth_eth_wire_types::{EthMessage, EthNetworkPrimitives, NetworkPrimitives};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
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

impl RawCapabilityMessage {
    /// Creates a new capability message with the given id and payload.
    pub const fn new(id: usize, payload: Bytes) -> Self {
        Self { id, payload }
    }

    /// Creates a raw message for the eth sub-protocol.
    ///
    /// Caller must ensure that the rlp encoded `payload` matches the given `id`.
    ///
    /// See also  [`EthMessage`]
    pub const fn eth(id: EthMessageID, payload: Bytes) -> Self {
        Self::new(id as usize, payload)
    }
}

/// Various protocol related event types bubbled up from a session that need to be handled by the
/// network.
#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum CapabilityMessage<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Eth sub-protocol message.
    #[cfg_attr(
        feature = "serde",
        serde(bound = "EthMessage<N>: Serialize + serde::de::DeserializeOwned")
    )]
    Eth(EthMessage<N>),
    /// Any other or manually crafted eth message.
    Other(RawCapabilityMessage),
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
    pub const fn capability(&self) -> Cow<'_, Capability> {
        match self {
            Self::Eth { version, .. } => Cow::Owned(Capability::eth(*version)),
            Self::UnknownCapability { cap, .. } => Cow::Borrowed(cap),
        }
    }

    /// Returns the name of the capability.
    #[inline]
    pub fn name(&self) -> &str {
        match self {
            Self::Eth { .. } => "eth",
            Self::UnknownCapability { cap, .. } => cap.name.as_ref(),
        }
    }

    /// Returns true if the capability is eth.
    #[inline]
    pub const fn is_eth(&self) -> bool {
        matches!(self, Self::Eth { .. })
    }

    /// Returns the version of the capability.
    pub const fn version(&self) -> u8 {
        match self {
            Self::Eth { version, .. } => *version as u8,
            Self::UnknownCapability { cap, .. } => cap.version as u8,
        }
    }

    /// Returns the eth version if it's the `eth` capability.
    pub const fn eth_version(&self) -> Option<EthVersion> {
        match self {
            Self::Eth { version, .. } => Some(*version),
            _ => None,
        }
    }

    /// Returns the message ID offset of the current capability.
    ///
    /// This represents the message ID offset for the first message of the eth capability in the
    /// message id space.
    pub const fn message_id_offset(&self) -> u8 {
        match self {
            Self::Eth { offset, .. } | Self::UnknownCapability { offset, .. } => *offset,
        }
    }

    /// Returns the message ID offset of the current capability relative to the start of the
    /// reserved message id space: [`MAX_RESERVED_MESSAGE_ID`].
    pub const fn relative_message_id_offset(&self) -> u8 {
        self.message_id_offset() - MAX_RESERVED_MESSAGE_ID - 1
    }

    /// Returns the number of protocol messages supported by this capability.
    pub const fn num_messages(&self) -> u8 {
        match self {
            Self::Eth { version: _version, .. } => EthMessageID::max() + 1,
            Self::UnknownCapability { messages, .. } => *messages,
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
    #[inline]
    pub fn try_new(
        local_protocols: Vec<Protocol>,
        peer_capabilities: Vec<Capability>,
    ) -> Result<Self, P2PStreamError> {
        shared_capability_offsets(local_protocols, peer_capabilities).map(Self)
    }

    /// Iterates over the shared capabilities.
    #[inline]
    pub fn iter_caps(&self) -> impl Iterator<Item = &SharedCapability> {
        self.0.iter()
    }

    /// Returns the eth capability if it is shared.
    #[inline]
    pub fn eth(&self) -> Result<&SharedCapability, P2PStreamError> {
        self.iter_caps().find(|c| c.is_eth()).ok_or(P2PStreamError::CapabilityNotShared)
    }

    /// Returns the negotiated eth version if it is shared.
    #[inline]
    pub fn eth_version(&self) -> Result<EthVersion, P2PStreamError> {
        self.iter_caps()
            .find_map(SharedCapability::eth_version)
            .ok_or(P2PStreamError::CapabilityNotShared)
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

    /// Returns the number of shared capabilities.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns true if there are no shared capabilities.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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
    let mut shared_capabilities: HashMap<_, ProtoVersion> = HashMap::default();

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
            if shared_capabilities
                .get(&peer_capability.name)
                .map_or(true, |v| peer_capability.version > v.version)
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
        let proto_version = &shared_capabilities[&name];
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
    /// Thrown when the message id for a [`SharedCapability`] overlaps with the reserved p2p
    /// message id space [`MAX_RESERVED_MESSAGE_ID`].
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
    use crate::{Capabilities, Capability};

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
    fn test_peer_capability_version_zero() {
        let cap = Capability::new_static("TestName", 0);
        let local_capabilities: Vec<Protocol> =
            vec![Protocol::new(cap.clone(), 0), EthVersion::Eth67.into(), EthVersion::Eth68.into()];
        let peer_capabilities = vec![cap.clone()];

        let shared = shared_capability_offsets(local_capabilities, peer_capabilities).unwrap();
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0], SharedCapability::UnknownCapability { cap, offset: 16, messages: 0 })
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
