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

    /// Returns the corresponding snap capability for the given version.
    pub const fn snap(version: SnapVersion) -> Self {
        let cap = Capability::snap(version);
        let messages = version.message_count();
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

    /// Returns the `snap/2` capability.
    pub const fn snap_2() -> Self {
        Self::snap(SnapVersion::V2)
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

/// Local limits for inbound messages of an `RLPx` subprotocol.
///
/// These limits are not advertised to the remote peer and do not affect message ID negotiation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolIngressLimits {
    max_frame_bytes: Option<usize>,
    max_buffered_bytes: usize,
    max_buffered_messages: usize,
}

impl ProtocolIngressLimits {
    /// Default byte budget for messages waiting to be polled by a protocol connection.
    pub const DEFAULT_MAX_BUFFERED_BYTES: usize = 32 * 1024 * 1024;

    /// Default message budget for messages waiting to be polled by a protocol connection.
    pub const DEFAULT_MAX_BUFFERED_MESSAGES: usize = 1024;

    /// Creates limits with an explicit maximum inbound frame size.
    ///
    /// The frame size includes the capability-local message ID byte.
    ///
    /// # Panics
    ///
    /// Panics if `max_frame_bytes` is zero.
    pub const fn new(max_frame_bytes: usize) -> Self {
        assert!(max_frame_bytes > 0, "maximum frame size must be non-zero");
        Self { max_frame_bytes: Some(max_frame_bytes), ..Self::default_values() }
    }

    /// Sets the maximum number of buffered frame bytes.
    ///
    /// # Panics
    ///
    /// Panics if `max_buffered_bytes` is zero.
    pub const fn with_max_buffered_bytes(mut self, max_buffered_bytes: usize) -> Self {
        assert!(max_buffered_bytes > 0, "maximum buffered bytes must be non-zero");
        self.max_buffered_bytes = max_buffered_bytes;
        self
    }

    /// Sets the maximum number of buffered messages.
    ///
    /// # Panics
    ///
    /// Panics if `max_buffered_messages` is zero.
    pub const fn with_max_buffered_messages(mut self, max_buffered_messages: usize) -> Self {
        assert!(max_buffered_messages > 0, "maximum buffered messages must be non-zero");
        self.max_buffered_messages = max_buffered_messages;
        self
    }

    /// Returns the explicit maximum inbound frame size, if configured.
    pub const fn max_frame_bytes(&self) -> Option<usize> {
        self.max_frame_bytes
    }

    /// Returns the maximum number of buffered frame bytes.
    pub const fn max_buffered_bytes(&self) -> usize {
        self.max_buffered_bytes
    }

    /// Returns the maximum number of buffered messages.
    pub const fn max_buffered_messages(&self) -> usize {
        self.max_buffered_messages
    }

    const fn default_values() -> Self {
        Self {
            max_frame_bytes: None,
            max_buffered_bytes: Self::DEFAULT_MAX_BUFFERED_BYTES,
            max_buffered_messages: Self::DEFAULT_MAX_BUFFERED_MESSAGES,
        }
    }
}

impl Default for ProtocolIngressLimits {
    fn default() -> Self {
        Self::default_values()
    }
}

impl From<EthVersion> for Protocol {
    fn from(version: EthVersion) -> Self {
        Self::eth(version)
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
        // Test that Protocol::eth() returns correct message counts for each version
        // This ensures that EthMessageID::message_count() produces the expected results
        assert_eq!(Protocol::eth(EthVersion::Eth66).messages(), 17);
        assert_eq!(Protocol::eth(EthVersion::Eth67).messages(), 17);
        assert_eq!(Protocol::eth(EthVersion::Eth68).messages(), 17);
        assert_eq!(Protocol::eth(EthVersion::Eth69).messages(), 18);
        assert_eq!(Protocol::eth(EthVersion::Eth70).messages(), 18);
        assert_eq!(Protocol::eth(EthVersion::Eth71).messages(), 20);
        assert_eq!(Protocol::snap(SnapVersion::V2).messages(), 10);
    }
}
