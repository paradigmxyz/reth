use crate::{Capability, EthVersion, ProtocolVersion};
use alloy_rlp::{RlpDecodable, RlpEncodable};
use reth_codecs::add_arbitrary_tests;
use reth_network_peers::PeerId;
use reth_primitives::constants::RETH_CLIENT_VERSION;

/// The default tcp port for p2p.
///
/// Note: this is the same as discovery port: `DEFAULT_DISCOVERY_PORT`
pub(crate) const DEFAULT_TCP_PORT: u16 = 30303;

use crate::protocol::Protocol;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// This is a superset of [`HelloMessage`] that provides additional protocol [Protocol] information
/// about the number of messages used by each capability in order to do proper message ID
/// multiplexing.
///
/// This type is required for the `p2p` handshake because the [`HelloMessage`] does not share the
/// number of messages used by each capability.
///
/// To get the encodable [`HelloMessage`] without the additional protocol information, use the
/// [`HelloMessageWithProtocols::message`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct HelloMessageWithProtocols {
    /// The version of the `p2p` protocol.
    pub protocol_version: ProtocolVersion,
    /// Specifies the client software identity, as a human-readable string (e.g.
    /// "Ethereum(++)/1.0.0").
    pub client_version: String,
    /// The list of supported capabilities and their versions.
    pub protocols: Vec<Protocol>,
    /// The port that the client is listening on, zero indicates the client is not listening.
    ///
    /// By default this is `30303` which is the same as the default discovery port.
    pub port: u16,
    /// The secp256k1 public key corresponding to the node's private key.
    pub id: PeerId,
}

impl HelloMessageWithProtocols {
    /// Starts a new `HelloMessageProtocolsBuilder`
    ///
    /// ```
    /// use reth_eth_wire::HelloMessageWithProtocols;
    /// use reth_network_peers::pk2id;
    /// use secp256k1::{SecretKey, SECP256K1};
    /// let secret_key = SecretKey::new(&mut rand::thread_rng());
    /// let id = pk2id(&secret_key.public_key(SECP256K1));
    /// let status = HelloMessageWithProtocols::builder(id).build();
    /// ```
    pub const fn builder(id: PeerId) -> HelloMessageBuilder {
        HelloMessageBuilder::new(id)
    }

    /// Returns the raw [`HelloMessage`] without the additional protocol information.
    #[inline]
    pub fn message(&self) -> HelloMessage {
        HelloMessage {
            protocol_version: self.protocol_version,
            client_version: self.client_version.clone(),
            capabilities: self.protocols.iter().map(|p| p.cap.clone()).collect(),
            port: self.port,
            id: self.id,
        }
    }

    /// Converts the type into a [`HelloMessage`] without the additional protocol information.
    pub fn into_message(self) -> HelloMessage {
        HelloMessage {
            protocol_version: self.protocol_version,
            client_version: self.client_version,
            capabilities: self.protocols.into_iter().map(|p| p.cap).collect(),
            port: self.port,
            id: self.id,
        }
    }

    /// Returns true if the set of protocols contains the given protocol.
    #[inline]
    pub fn contains_protocol(&self, protocol: &Protocol) -> bool {
        self.protocols.iter().any(|p| p.cap == protocol.cap)
    }

    /// Adds a new protocol to the set.
    ///
    /// Returns an error if the protocol already exists.
    #[inline]
    pub fn try_add_protocol(&mut self, protocol: Protocol) -> Result<(), Protocol> {
        if self.contains_protocol(&protocol) {
            Err(protocol)
        } else {
            self.protocols.push(protocol);
            Ok(())
        }
    }
}

// TODO: determine if we should allow for the extra fields at the end like EIP-706 suggests
/// Raw rlpx protocol message used in the `p2p` handshake, containing information about the
/// supported RLPx protocol version and capabilities.
///
/// See also <https://github.com/ethereum/devp2p/blob/master/rlpx.md#hello-0x00>
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct HelloMessage {
    /// The version of the `p2p` protocol.
    pub protocol_version: ProtocolVersion,
    /// Specifies the client software identity, as a human-readable string (e.g.
    /// "Ethereum(++)/1.0.0").
    pub client_version: String,
    /// The list of supported capabilities and their versions.
    pub capabilities: Vec<Capability>,
    /// The port that the client is listening on, zero indicates the client is not listening.
    pub port: u16,
    /// The secp256k1 public key corresponding to the node's private key.
    pub id: PeerId,
}

// === impl HelloMessage ===

impl HelloMessage {
    /// Starts a new `HelloMessageBuilder`
    ///
    /// ```
    /// use reth_eth_wire::HelloMessage;
    /// use reth_network_peers::pk2id;
    /// use secp256k1::{SecretKey, SECP256K1};
    /// let secret_key = SecretKey::new(&mut rand::thread_rng());
    /// let id = pk2id(&secret_key.public_key(SECP256K1));
    /// let status = HelloMessage::builder(id).build();
    /// ```
    pub const fn builder(id: PeerId) -> HelloMessageBuilder {
        HelloMessageBuilder::new(id)
    }
}

/// Builder for [`HelloMessageWithProtocols`]
#[derive(Debug)]
pub struct HelloMessageBuilder {
    /// The version of the `p2p` protocol.
    pub protocol_version: Option<ProtocolVersion>,
    /// Specifies the client software identity, as a human-readable string (e.g.
    /// "Ethereum(++)/1.0.0").
    pub client_version: Option<String>,
    /// The list of supported protocols.
    pub protocols: Option<Vec<Protocol>>,
    /// The port that the client is listening on, zero indicates the client is not listening.
    pub port: Option<u16>,
    /// The secp256k1 public key corresponding to the node's private key.
    pub id: PeerId,
}

// === impl HelloMessageBuilder ===

impl HelloMessageBuilder {
    /// Create a new builder to configure a [`HelloMessage`]
    pub const fn new(id: PeerId) -> Self {
        Self { protocol_version: None, client_version: None, protocols: None, port: None, id }
    }

    /// Sets the port the client is listening on
    pub const fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Adds a new protocol to use.
    pub fn protocol(mut self, protocols: impl Into<Protocol>) -> Self {
        self.protocols.get_or_insert_with(Vec::new).push(protocols.into());
        self
    }

    /// Sets protocols to use.
    pub fn protocols(mut self, protocols: impl IntoIterator<Item = Protocol>) -> Self {
        self.protocols.get_or_insert_with(Vec::new).extend(protocols);
        self
    }

    /// Sets client version.
    pub fn client_version(mut self, client_version: impl Into<String>) -> Self {
        self.client_version = Some(client_version.into());
        self
    }

    /// Sets protocol version.
    pub const fn protocol_version(mut self, protocol_version: ProtocolVersion) -> Self {
        self.protocol_version = Some(protocol_version);
        self
    }

    /// Consumes the type and returns the configured [`HelloMessage`]
    ///
    /// Unset fields will be set to their default values:
    /// - `protocol_version`: [`ProtocolVersion::V5`]
    /// - `client_version`: [`RETH_CLIENT_VERSION`]
    /// - `capabilities`: All [`EthVersion`]
    pub fn build(self) -> HelloMessageWithProtocols {
        let Self { protocol_version, client_version, protocols, port, id } = self;
        HelloMessageWithProtocols {
            protocol_version: protocol_version.unwrap_or_default(),
            client_version: client_version.unwrap_or_else(|| RETH_CLIENT_VERSION.to_string()),
            protocols: protocols.unwrap_or_else(|| {
                vec![EthVersion::Eth68.into(), EthVersion::Eth67.into(), EthVersion::Eth66.into()]
            }),
            port: port.unwrap_or(DEFAULT_TCP_PORT),
            id,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{p2pstream::P2PMessage, Capability, EthVersion, HelloMessage, ProtocolVersion};
    use alloy_rlp::{Decodable, Encodable, EMPTY_STRING_CODE};
    use reth_network_peers::pk2id;
    use secp256k1::{SecretKey, SECP256K1};

    #[test]
    fn test_hello_encoding_round_trip() {
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let id = pk2id(&secret_key.public_key(SECP256K1));
        let hello = P2PMessage::Hello(HelloMessage {
            protocol_version: ProtocolVersion::V5,
            client_version: "reth/0.1.0".to_string(),
            capabilities: vec![Capability::new_static("eth", EthVersion::Eth67 as usize)],
            port: 30303,
            id,
        });

        let mut hello_encoded = Vec::new();
        hello.encode(&mut hello_encoded);

        let hello_decoded = P2PMessage::decode(&mut &hello_encoded[..]).unwrap();

        assert_eq!(hello, hello_decoded);
    }

    #[test]
    fn hello_encoding_length() {
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let id = pk2id(&secret_key.public_key(SECP256K1));
        let hello = P2PMessage::Hello(HelloMessage {
            protocol_version: ProtocolVersion::V5,
            client_version: "reth/0.1.0".to_string(),
            capabilities: vec![Capability::new_static("eth", EthVersion::Eth67 as usize)],
            port: 30303,
            id,
        });

        let mut hello_encoded = Vec::new();
        hello.encode(&mut hello_encoded);

        assert_eq!(hello_encoded.len(), hello.length());
    }

    #[test]
    fn hello_message_id_prefix() {
        // ensure that the hello message id is prefixed
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let id = pk2id(&secret_key.public_key(SECP256K1));
        let hello = P2PMessage::Hello(HelloMessage {
            protocol_version: ProtocolVersion::V5,
            client_version: "reth/0.1.0".to_string(),
            capabilities: vec![Capability::new_static("eth", EthVersion::Eth67 as usize)],
            port: 30303,
            id,
        });

        let mut hello_encoded = Vec::new();
        hello.encode(&mut hello_encoded);

        // zero is encoded as 0x80, the empty string code in RLP
        assert_eq!(hello_encoded[0], EMPTY_STRING_CODE);
    }
}
