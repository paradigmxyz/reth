//! A Protocol defines a P2P subprotocol in a RLPx connection

use crate::{capability::Capability, EthVersion};

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
    pub messages: u8,
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
}

impl From<EthVersion> for Protocol {
    fn from(version: EthVersion) -> Self {
        Self::eth(version)
    }
}
