//! A Protocol defines a P2P subprotocol in an `RLPx` connection

use crate::{Capability, EthMessageID, EthVersion, SnapVersion};

/// Type that represents a [Capability] and the number of messages it uses.
///
/// Only the [Capability] is shared with the remote peer, assuming both parties know the number of
/// messages used by the protocol which is used for message ID multiplexing.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Protocol {
    /// The name of the subprotocol
    pub cap: Capability,
    /// The number of messages used/reserved by this protocol
    ///
    /// This is used for message ID multiplexing
    messages: u8,
}

impl Protocol {
    /// Create a new protocol with the given name and number of messages
    pub const fn new(cap: Capability, messages: u8) -> Self {
        Self { cap, messages }
    }

    /// Returns the corresponding eth capability for the given version.
    pub const fn eth(version: EthVersion) -> Self {
        let cap = Capability::eth(version);
        let messages = EthMessageID::message_count(version);
        Self::new(cap, messages)
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

    /// Returns the corresponding snap capability for the given version.
    ///
    /// All snap protocol versions use 8 message IDs (0x00–0x07).
    pub const fn snap(version: SnapVersion) -> Self {
        let cap = Capability::snap(version);
        Self::new(cap, 8)
    }

    /// Returns the [`SnapVersion::Snap1`] capability.
    pub const fn snap_1() -> Self {
        Self::snap(SnapVersion::Snap1)
    }

    /// Returns the [`SnapVersion::Snap2`] capability.
    pub const fn snap_2() -> Self {
        Self::snap(SnapVersion::Snap2)
    }

    /// Consumes the type and returns a tuple of the [Capability] and number of messages.
    #[inline]
    pub(crate) fn split(self) -> (Capability, u8) {
        (self.cap, self.messages)
    }

    /// The number of values needed to represent all message IDs of capability.
    pub const fn messages(&self) -> u8 {
        self.messages
    }
}

impl From<EthVersion> for Protocol {
    fn from(version: EthVersion) -> Self {
        Self::eth(version)
    }
}

impl From<SnapVersion> for Protocol {
    fn from(version: SnapVersion) -> Self {
        Self::snap(version)
    }
}

/// A helper type to keep track of the protocol version and number of messages used by the protocol.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ProtoVersion {
    /// Number of messages for a protocol
    pub(crate) messages: u8,
    /// Version of the protocol
    pub(crate) version: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_eth_message_count() {
        assert_eq!(Protocol::eth(EthVersion::Eth66).messages(), 17);
        assert_eq!(Protocol::eth(EthVersion::Eth67).messages(), 17);
        assert_eq!(Protocol::eth(EthVersion::Eth68).messages(), 17);
        assert_eq!(Protocol::eth(EthVersion::Eth69).messages(), 18);
        assert_eq!(Protocol::eth(EthVersion::Eth70).messages(), 18);
        assert_eq!(Protocol::eth(EthVersion::Eth71).messages(), 20);
    }

    #[test]
    fn test_protocol_snap_message_count() {
        assert_eq!(Protocol::snap(SnapVersion::Snap1).messages(), 8);
        assert_eq!(Protocol::snap(SnapVersion::Snap2).messages(), 8);
    }
}
