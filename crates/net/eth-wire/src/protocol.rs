//! A Protocol defines a P2P subprotocol in a RLPx connection

use crate::{capability::Capability, EthMessageID, EthVersion};

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
        let messages = version.total_messages();
        Self::new(cap, messages)
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

    /// Consumes the type and returns a tuple of the [Capability] and number of messages.
    #[inline]
    pub(crate) fn split(self) -> (Capability, u8) {
        (self.cap, self.messages)
    }

    /// The number of values needed to represent all message IDs of capability.
    pub fn messages(&self) -> u8 {
        if self.cap.is_eth() {
            return EthMessageID::max() + 1
        }
        self.messages
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
