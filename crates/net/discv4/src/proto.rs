//! Discovery v4 protocol implementation.

use crate::{error::DecodePacketError, MAX_PACKET_SIZE, MIN_PACKET_SIZE};
use alloy_primitives::{
    bytes::{Buf, BufMut, Bytes, BytesMut},
    keccak256, B256,
};
use alloy_rlp::{
    Decodable, Encodable, Error as RlpError, Header, RlpDecodable, RlpEncodable,
    RlpEncodableWrapper,
};
use enr::Enr;
use reth_ethereum_forks::{EnrForkIdEntry, ForkId};
use reth_network_peers::{pk2id, NodeRecord, PeerId};
use secp256k1::{
    ecdsa::{RecoverableSignature, RecoveryId},
    SecretKey, SECP256K1,
};
use std::net::{IpAddr, Ipv4Addr};

// Note: this is adapted from https://github.com/vorot93/discv4

/// Represents the identifier for message variants.
///
/// This enumeration assigns unique identifiers (u8 values) to different message types.
#[derive(Debug)]
#[repr(u8)]
pub enum MessageId {
    /// Ping message identifier.
    Ping = 1,
    /// Pong message identifier.
    Pong = 2,
    /// Find node message identifier.
    FindNode = 3,
    /// Neighbours message identifier.
    Neighbours = 4,
    /// ENR request message identifier.
    EnrRequest = 5,
    /// ENR response message identifier.
    EnrResponse = 6,
}

impl MessageId {
    /// Converts the byte that represents the message id to the enum.
    const fn from_u8(msg: u8) -> Result<Self, u8> {
        Ok(match msg {
            1 => Self::Ping,
            2 => Self::Pong,
            3 => Self::FindNode,
            4 => Self::Neighbours,
            5 => Self::EnrRequest,
            6 => Self::EnrResponse,
            _ => return Err(msg),
        })
    }
}

/// Enum representing various message types exchanged in the Discovery v4 protocol.
#[derive(Debug, Eq, PartialEq)]
pub enum Message {
    /// Represents a ping message sent during liveness checks.
    Ping(Ping),
    /// Represents a pong message, which is a reply to a PING message.
    Pong(Pong),
    /// Represents a query for nodes in the given bucket.
    FindNode(FindNode),
    /// Represents a neighbour message, providing information about nearby nodes.
    Neighbours(Neighbours),
    /// Represents an ENR request message, a request for Ethereum Node Records (ENR) as per [EIP-778](https://eips.ethereum.org/EIPS/eip-778).
    EnrRequest(EnrRequest),
    /// Represents an ENR response message, a response to an ENR request with Ethereum Node Records (ENR) as per [EIP-778](https://eips.ethereum.org/EIPS/eip-778).
    EnrResponse(EnrResponse),
}

// === impl Message ===

impl Message {
    /// Returns the id for this type
    pub const fn msg_type(&self) -> MessageId {
        match self {
            Self::Ping(_) => MessageId::Ping,
            Self::Pong(_) => MessageId::Pong,
            Self::FindNode(_) => MessageId::FindNode,
            Self::Neighbours(_) => MessageId::Neighbours,
            Self::EnrRequest(_) => MessageId::EnrRequest,
            Self::EnrResponse(_) => MessageId::EnrResponse,
        }
    }

    /// Encodes the UDP datagram, See <https://github.com/ethereum/devp2p/blob/master/discv4.md#wire-protocol>
    ///
    /// The datagram is `header || payload`
    /// where header is `hash || signature || packet-type`
    pub fn encode(&self, secret_key: &SecretKey) -> (Bytes, B256) {
        // allocate max packet size
        let mut datagram = BytesMut::with_capacity(MAX_PACKET_SIZE);

        // since signature has fixed len, we can split and fill the datagram buffer at fixed
        // positions, this way we can encode the message directly in the datagram buffer
        let mut sig_bytes = datagram.split_off(B256::len_bytes());
        let mut payload = sig_bytes.split_off(secp256k1::constants::COMPACT_SIGNATURE_SIZE + 1);

        // Put the message type at the beginning of the payload
        payload.put_u8(self.msg_type() as u8);

        // Match the message type and encode the corresponding message into the payload
        match self {
            Self::Ping(message) => message.encode(&mut payload),
            Self::Pong(message) => message.encode(&mut payload),
            Self::FindNode(message) => message.encode(&mut payload),
            Self::Neighbours(message) => message.encode(&mut payload),
            Self::EnrRequest(message) => message.encode(&mut payload),
            Self::EnrResponse(message) => message.encode(&mut payload),
        }

        // Sign the payload with the secret key using recoverable ECDSA
        let signature: RecoverableSignature = SECP256K1.sign_ecdsa_recoverable(
            &secp256k1::Message::from_digest(keccak256(&payload).0),
            secret_key,
        );

        // Serialize the signature and append it to the signature bytes
        let (rec, sig) = signature.serialize_compact();
        sig_bytes.extend_from_slice(&sig);
        sig_bytes.put_u8(rec.to_i32() as u8);
        sig_bytes.unsplit(payload);

        // Calculate the hash of the signature bytes and append it to the datagram
        let hash = keccak256(&sig_bytes);
        datagram.extend_from_slice(hash.as_slice());

        // Append the signature bytes to the datagram
        datagram.unsplit(sig_bytes);

        // Return the frozen datagram and the hash
        (datagram.freeze(), hash)
    }

    /// Decodes the [`Message`] from the given buffer.
    ///
    /// Returns the decoded message and the public key of the sender.
    pub fn decode(packet: &[u8]) -> Result<Packet, DecodePacketError> {
        if packet.len() < MIN_PACKET_SIZE {
            return Err(DecodePacketError::PacketTooShort)
        }

        // parses the wire-protocol, every packet starts with a header:
        // packet-header = hash || signature || packet-type
        // hash = keccak256(signature || packet-type || packet-data)
        // signature = sign(packet-type || packet-data)

        let header_hash = keccak256(&packet[32..]);
        let data_hash = B256::from_slice(&packet[..32]);
        if data_hash != header_hash {
            return Err(DecodePacketError::HashMismatch)
        }

        let signature = &packet[32..96];
        let recovery_id = RecoveryId::from_i32(packet[96] as i32)?;
        let recoverable_sig = RecoverableSignature::from_compact(signature, recovery_id)?;

        // recover the public key
        let msg = secp256k1::Message::from_digest(keccak256(&packet[97..]).0);

        let pk = SECP256K1.recover_ecdsa(&msg, &recoverable_sig)?;
        let node_id = pk2id(&pk);

        let msg_type = packet[97];
        let payload = &mut &packet[98..];

        let msg = match MessageId::from_u8(msg_type).map_err(DecodePacketError::UnknownMessage)? {
            MessageId::Ping => Self::Ping(Ping::decode(payload)?),
            MessageId::Pong => Self::Pong(Pong::decode(payload)?),
            MessageId::FindNode => Self::FindNode(FindNode::decode(payload)?),
            MessageId::Neighbours => Self::Neighbours(Neighbours::decode(payload)?),
            MessageId::EnrRequest => Self::EnrRequest(EnrRequest::decode(payload)?),
            MessageId::EnrResponse => Self::EnrResponse(EnrResponse::decode(payload)?),
        };

        Ok(Packet { msg, node_id, hash: header_hash })
    }
}

/// Represents a decoded packet.
///
/// This struct holds information about a decoded packet, including the message, node ID, and hash.
#[derive(Debug)]
pub struct Packet {
    /// The decoded message from the packet.
    pub msg: Message,
    /// The ID of the peer that sent the packet.
    pub node_id: PeerId,
    /// The hash of the packet.
    pub hash: B256,
}

/// Represents the `from` field in the `Ping` packet
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, RlpEncodableWrapper)]
struct PingNodeEndpoint(NodeEndpoint);

impl alloy_rlp::Decodable for PingNodeEndpoint {
    #[inline]
    fn decode(b: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let alloy_rlp::Header { list, payload_length } = alloy_rlp::Header::decode(b)?;
        if !list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let started_len = b.len();
        if started_len < payload_length {
            return Err(alloy_rlp::Error::InputTooShort);
        }

        // Geth allows the ipaddr to be possibly empty:
        // <https://github.com/ethereum/go-ethereum/blob/380688c636a654becc8f114438c2a5d93d2db032/p2p/discover/v4_udp.go#L206-L209>
        // <https://github.com/ethereum/go-ethereum/blob/380688c636a654becc8f114438c2a5d93d2db032/p2p/enode/node.go#L189-L189>
        //
        // Therefore, if we see an empty list instead of a properly formed `IpAddr`, we will
        // instead use `IpV4Addr::UNSPECIFIED`
        let address =
            if *b.first().ok_or(alloy_rlp::Error::InputTooShort)? == alloy_rlp::EMPTY_STRING_CODE {
                let addr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
                b.advance(1);
                addr
            } else {
                alloy_rlp::Decodable::decode(b)?
            };

        let this = NodeEndpoint {
            address,
            udp_port: alloy_rlp::Decodable::decode(b)?,
            tcp_port: alloy_rlp::Decodable::decode(b)?,
        };
        let consumed = started_len - b.len();
        if consumed != payload_length {
            return Err(alloy_rlp::Error::ListLengthMismatch {
                expected: payload_length,
                got: consumed,
            });
        }
        Ok(Self(this))
    }
}

/// Represents the `from`, `to` fields in the packets
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, RlpEncodable, RlpDecodable)]
pub struct NodeEndpoint {
    /// The IP address of the network endpoint. It can be either IPv4 or IPv6.
    pub address: IpAddr,
    /// The UDP port used for communication in the discovery protocol.
    pub udp_port: u16,
    /// The TCP port used for communication in the `RLPx` protocol.
    pub tcp_port: u16,
}

impl From<NodeRecord> for NodeEndpoint {
    fn from(NodeRecord { address, tcp_port, udp_port, .. }: NodeRecord) -> Self {
        Self { address, tcp_port, udp_port }
    }
}

impl NodeEndpoint {
    /// Creates a new [`NodeEndpoint`] from a given UDP address and TCP port.
    pub const fn from_udp_address(udp_address: &std::net::SocketAddr, tcp_port: u16) -> Self {
        Self { address: udp_address.ip(), udp_port: udp_address.port(), tcp_port }
    }
}

/// A [FindNode packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#findnode-packet-0x03).
#[derive(Clone, Copy, Debug, Eq, PartialEq, RlpEncodable)]
pub struct FindNode {
    /// The target node's ID, a 64-byte secp256k1 public key.
    pub id: PeerId,
    /// The expiration timestamp of the packet, an absolute UNIX time stamp.
    pub expire: u64,
}

impl Decodable for FindNode {
    // NOTE(onbjerg): Manual implementation to satisfy EIP-8.
    //
    // See https://eips.ethereum.org/EIPS/eip-8
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let b = &mut &**buf;
        let rlp_head = Header::decode(b)?;
        if !rlp_head.list {
            return Err(RlpError::UnexpectedString)
        }
        let started_len = b.len();

        let this = Self { id: Decodable::decode(b)?, expire: Decodable::decode(b)? };

        // NOTE(onbjerg): Because of EIP-8, we only check that we did not consume *more* than the
        // payload length, i.e. it is ok if payload length is greater than what we consumed, as we
        // just discard the remaining list items
        let consumed = started_len - b.len();
        if consumed > rlp_head.payload_length {
            return Err(RlpError::ListLengthMismatch {
                expected: rlp_head.payload_length,
                got: consumed,
            })
        }

        let rem = rlp_head.payload_length - consumed;
        b.advance(rem);
        *buf = *b;

        Ok(this)
    }
}

/// A [Neighbours packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#neighbors-packet-0x04).
#[derive(Clone, Debug, Eq, PartialEq, RlpEncodable)]
pub struct Neighbours {
    /// The list of nodes containing IP, UDP port, TCP port, and node ID.
    pub nodes: Vec<NodeRecord>,
    /// The expiration timestamp of the packet, an absolute UNIX time stamp.
    pub expire: u64,
}

impl Decodable for Neighbours {
    // NOTE(onbjerg): Manual implementation to satisfy EIP-8.
    //
    // See https://eips.ethereum.org/EIPS/eip-8
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let b = &mut &**buf;
        let rlp_head = Header::decode(b)?;
        if !rlp_head.list {
            return Err(RlpError::UnexpectedString)
        }
        let started_len = b.len();

        let this = Self { nodes: Decodable::decode(b)?, expire: Decodable::decode(b)? };

        // NOTE(onbjerg): Because of EIP-8, we only check that we did not consume *more* than the
        // payload length, i.e. it is ok if payload length is greater than what we consumed, as we
        // just discard the remaining list items
        let consumed = started_len - b.len();
        if consumed > rlp_head.payload_length {
            return Err(RlpError::ListLengthMismatch {
                expected: rlp_head.payload_length,
                got: consumed,
            })
        }

        let rem = rlp_head.payload_length - consumed;
        b.advance(rem);
        *buf = *b;

        Ok(this)
    }
}

/// A [ENRRequest packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#enrrequest-packet-0x05).
///
/// This packet is used to request the current version of a node's Ethereum Node Record (ENR).
#[derive(Clone, Copy, Debug, Eq, PartialEq, RlpEncodable)]
pub struct EnrRequest {
    /// The expiration timestamp for the request. No reply should be sent if it refers to a time in
    /// the past.
    pub expire: u64,
}

impl Decodable for EnrRequest {
    // NOTE(onbjerg): Manual implementation to satisfy EIP-8.
    //
    // See https://eips.ethereum.org/EIPS/eip-8
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let b = &mut &**buf;
        let rlp_head = Header::decode(b)?;
        if !rlp_head.list {
            return Err(RlpError::UnexpectedString)
        }
        let started_len = b.len();

        let this = Self { expire: Decodable::decode(b)? };

        // NOTE(onbjerg): Because of EIP-8, we only check that we did not consume *more* than the
        // payload length, i.e. it is ok if payload length is greater than what we consumed, as we
        // just discard the remaining list items
        let consumed = started_len - b.len();
        if consumed > rlp_head.payload_length {
            return Err(RlpError::ListLengthMismatch {
                expected: rlp_head.payload_length,
                got: consumed,
            })
        }

        let rem = rlp_head.payload_length - consumed;
        b.advance(rem);
        *buf = *b;

        Ok(this)
    }
}

/// A [ENRResponse packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#enrresponse-packet-0x06).
///
/// This packet is used to respond to an `ENRRequest` packet and includes the requested ENR along
/// with the hash of the original request.
#[derive(Clone, Debug, Eq, PartialEq, RlpEncodable, RlpDecodable)]
pub struct EnrResponse {
    /// The hash of the `ENRRequest` packet being replied to.
    pub request_hash: B256,
    /// The ENR (Ethereum Node Record) for the responding node.
    pub enr: Enr<SecretKey>,
}

// === impl EnrResponse ===

impl EnrResponse {
    /// Returns the [`ForkId`] if set
    ///
    /// See also <https://github.com/ethereum/go-ethereum/blob/9244d5cd61f3ea5a7645fdf2a1a96d53421e412f/eth/protocols/eth/discovery.go#L36>
    pub fn eth_fork_id(&self) -> Option<ForkId> {
        let mut maybe_fork_id = self.enr.get_raw_rlp(b"eth")?;
        EnrForkIdEntry::decode(&mut maybe_fork_id).ok().map(Into::into)
    }
}

/// Represents a Ping packet.
///
/// A [Ping packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#ping-packet-0x01).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Ping {
    /// The sender's endpoint.
    pub from: NodeEndpoint,
    /// The recipient's endpoint.
    pub to: NodeEndpoint,
    /// The expiration timestamp.
    pub expire: u64,
    /// Optional `enr_seq` for <https://eips.ethereum.org/EIPS/eip-868>
    pub enr_sq: Option<u64>,
}

impl Encodable for Ping {
    fn encode(&self, out: &mut dyn BufMut) {
        #[derive(RlpEncodable)]
        struct V4PingMessage<'a> {
            version: u32,
            from: &'a NodeEndpoint,
            to: &'a NodeEndpoint,
            expire: u64,
        }

        #[derive(RlpEncodable)]
        struct V4PingMessageEIP868<'a> {
            version: u32,
            from: &'a NodeEndpoint,
            to: &'a NodeEndpoint,
            expire: u64,
            enr_seq: u64,
        }
        if let Some(enr_seq) = self.enr_sq {
            V4PingMessageEIP868 {
                version: 4, // version 4
                from: &self.from,
                to: &self.to,
                expire: self.expire,
                enr_seq,
            }
            .encode(out);
        } else {
            V4PingMessage {
                version: 4, // version 4
                from: &self.from,
                to: &self.to,
                expire: self.expire,
            }
            .encode(out);
        }
    }
}

impl Decodable for Ping {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let b = &mut &**buf;
        let rlp_head = Header::decode(b)?;
        if !rlp_head.list {
            return Err(RlpError::UnexpectedString)
        }
        let started_len = b.len();

        // > Implementations should ignore any mismatches in version:
        // <https://github.com/ethereum/devp2p/blob/master/discv4.md#ping-packet-0x01>
        let _version = u32::decode(b)?;

        // see `Decodable` implementation in `PingNodeEndpoint` for why this is needed
        let from = PingNodeEndpoint::decode(b)?.0;

        let mut this =
            Self { from, to: Decodable::decode(b)?, expire: Decodable::decode(b)?, enr_sq: None };

        // only decode the ENR sequence if there's more data in the datagram to decode else skip
        if b.has_remaining() {
            this.enr_sq = Some(Decodable::decode(b)?);
        }

        let consumed = started_len - b.len();
        if consumed > rlp_head.payload_length {
            return Err(RlpError::ListLengthMismatch {
                expected: rlp_head.payload_length,
                got: consumed,
            })
        }
        let rem = rlp_head.payload_length - consumed;
        b.advance(rem);
        *buf = *b;
        Ok(this)
    }
}

/// Represents a Pong packet.
///
/// A [Pong packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#pong-packet-0x02).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pong {
    /// The recipient's endpoint.
    pub to: NodeEndpoint,
    /// The hash of the corresponding ping packet.
    pub echo: B256,
    /// The expiration timestamp.
    pub expire: u64,
    /// Optional `enr_seq` for <https://eips.ethereum.org/EIPS/eip-868>
    pub enr_sq: Option<u64>,
}

impl Encodable for Pong {
    fn encode(&self, out: &mut dyn BufMut) {
        #[derive(RlpEncodable)]
        struct PongMessageEIP868<'a> {
            to: &'a NodeEndpoint,
            echo: &'a B256,
            expire: u64,
            enr_seq: u64,
        }

        #[derive(RlpEncodable)]
        struct PongMessage<'a> {
            to: &'a NodeEndpoint,
            echo: &'a B256,
            expire: u64,
        }

        if let Some(enr_seq) = self.enr_sq {
            PongMessageEIP868 { to: &self.to, echo: &self.echo, expire: self.expire, enr_seq }
                .encode(out);
        } else {
            PongMessage { to: &self.to, echo: &self.echo, expire: self.expire }.encode(out);
        }
    }
}

impl Decodable for Pong {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let b = &mut &**buf;
        let rlp_head = Header::decode(b)?;
        if !rlp_head.list {
            return Err(RlpError::UnexpectedString)
        }
        let started_len = b.len();
        let mut this = Self {
            to: Decodable::decode(b)?,
            echo: Decodable::decode(b)?,
            expire: Decodable::decode(b)?,
            enr_sq: None,
        };

        // only decode the ENR sequence if there's more data in the datagram to decode else skip
        if b.has_remaining() {
            this.enr_sq = Some(Decodable::decode(b)?);
        }

        let consumed = started_len - b.len();
        if consumed > rlp_head.payload_length {
            return Err(RlpError::ListLengthMismatch {
                expected: rlp_head.payload_length,
                got: consumed,
            })
        }
        let rem = rlp_head.payload_length - consumed;
        b.advance(rem);
        *buf = *b;

        Ok(this)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{rng_endpoint, rng_ipv4_record, rng_ipv6_record, rng_message},
        DEFAULT_DISCOVERY_PORT, SAFE_MAX_DATAGRAM_NEIGHBOUR_RECORDS,
    };
    use alloy_primitives::hex;
    use assert_matches::assert_matches;
    use enr::EnrPublicKey;
    use rand::{thread_rng, Rng, RngCore};
    use reth_ethereum_forks::ForkHash;

    #[test]
    fn test_endpoint_ipv_v4() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 4];
            rng.fill_bytes(&mut ip);
            let msg = NodeEndpoint {
                address: IpAddr::V4(ip.into()),
                tcp_port: rng.gen(),
                udp_port: rng.gen(),
            };

            let decoded = NodeEndpoint::decode(&mut alloy_rlp::encode(msg).as_slice()).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_endpoint_ipv_64() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 16];
            rng.fill_bytes(&mut ip);
            let msg = NodeEndpoint {
                address: IpAddr::V6(ip.into()),
                tcp_port: rng.gen(),
                udp_port: rng.gen(),
            };

            let decoded = NodeEndpoint::decode(&mut alloy_rlp::encode(msg).as_slice()).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_ping_message() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 16];
            rng.fill_bytes(&mut ip);
            let msg = Ping {
                from: rng_endpoint(&mut rng),
                to: rng_endpoint(&mut rng),
                expire: 0,
                enr_sq: None,
            };

            let decoded = Ping::decode(&mut alloy_rlp::encode(&msg).as_slice()).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_ping_message_with_enr() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 16];
            rng.fill_bytes(&mut ip);
            let msg = Ping {
                from: rng_endpoint(&mut rng),
                to: rng_endpoint(&mut rng),
                expire: 0,
                enr_sq: Some(rng.gen()),
            };

            let decoded = Ping::decode(&mut alloy_rlp::encode(&msg).as_slice()).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_pong_message() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 16];
            rng.fill_bytes(&mut ip);
            let msg = Pong {
                to: rng_endpoint(&mut rng),
                echo: rng.gen(),
                expire: rng.gen(),
                enr_sq: None,
            };

            let decoded = Pong::decode(&mut alloy_rlp::encode(&msg).as_slice()).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_pong_message_with_enr() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 16];
            rng.fill_bytes(&mut ip);
            let msg = Pong {
                to: rng_endpoint(&mut rng),
                echo: rng.gen(),
                expire: rng.gen(),
                enr_sq: Some(rng.gen()),
            };

            let decoded = Pong::decode(&mut alloy_rlp::encode(&msg).as_slice()).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_hash_mismatch() {
        let mut rng = thread_rng();
        let msg = rng_message(&mut rng);
        let (secret_key, _) = SECP256K1.generate_keypair(&mut rng);
        let (buf, _) = msg.encode(&secret_key);

        let mut buf_vec = buf.to_vec();
        buf_vec.push(0);
        match Message::decode(buf_vec.as_slice()).unwrap_err() {
            DecodePacketError::HashMismatch => {}
            err => {
                unreachable!("unexpected err {}", err)
            }
        }
    }

    #[test]
    fn neighbours_max_ipv4() {
        let mut rng = thread_rng();
        let msg = Message::Neighbours(Neighbours {
            nodes: std::iter::repeat_with(|| rng_ipv4_record(&mut rng)).take(16).collect(),
            expire: rng.gen(),
        });
        let (secret_key, _) = SECP256K1.generate_keypair(&mut rng);

        let (encoded, _) = msg.encode(&secret_key);
        // Assert that 16 nodes never fit into one packet
        assert!(encoded.len() > MAX_PACKET_SIZE, "{} {msg:?}", encoded.len());
    }

    #[test]
    fn neighbours_max_nodes() {
        let mut rng = thread_rng();
        for _ in 0..1000 {
            let msg = Message::Neighbours(Neighbours {
                nodes: std::iter::repeat_with(|| rng_ipv6_record(&mut rng))
                    .take(SAFE_MAX_DATAGRAM_NEIGHBOUR_RECORDS)
                    .collect(),
                expire: rng.gen(),
            });
            let (secret_key, _) = SECP256K1.generate_keypair(&mut rng);

            let (encoded, _) = msg.encode(&secret_key);
            assert!(encoded.len() <= MAX_PACKET_SIZE, "{} {msg:?}", encoded.len());

            let mut neighbours = Neighbours {
                nodes: std::iter::repeat_with(|| rng_ipv6_record(&mut rng))
                    .take(SAFE_MAX_DATAGRAM_NEIGHBOUR_RECORDS - 1)
                    .collect(),
                expire: rng.gen(),
            };
            neighbours.nodes.push(rng_ipv4_record(&mut rng));
            let msg = Message::Neighbours(neighbours);
            let (encoded, _) = msg.encode(&secret_key);
            assert!(encoded.len() <= MAX_PACKET_SIZE, "{} {msg:?}", encoded.len());
        }
    }

    #[test]
    fn test_encode_decode_message() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let msg = rng_message(&mut rng);
            let (secret_key, pk) = SECP256K1.generate_keypair(&mut rng);
            let sender_id = pk2id(&pk);

            let (buf, _) = msg.encode(&secret_key);

            let packet = Message::decode(buf.as_ref()).unwrap();

            assert_eq!(msg, packet.msg);
            assert_eq!(sender_id, packet.node_id);
        }
    }

    #[test]
    fn decode_pong_packet() {
        let packet = "2ad84c37327a06c2522cf7bc039621da89f68907441b755935bb308dc4cd17d6fe550e90329ad6a516ca7db18e08900067928a0dfa3b5c75d55a42c984497373698d98616662c048983ea85895ea2da765eabeb15525478384e106337bfd8ed50002f3c9843ed8cae682fd1c80a008ad4dead0922211df47593e7d837b2b23d13954285871ca23250ea594993ded84635690e5829670";
        let data = hex::decode(packet).unwrap();
        Message::decode(&data).unwrap();
    }
    #[test]
    fn decode_ping_packet() {
        let packet = "05ae5bf922cf2a93f97632a4ab0943dc252a0dab0c42d86dd62e5d91e1a0966e9b628fbf4763fdfbb928540460b797e6be2e7058a82f6083f6d2e7391bb021741459976d4152aa16bbee0c3609dcfac6668db1ef78b7ee9f8b4ced10dd5ae2900101df04cb8403d12d4f82765f82765fc9843ed8cae6828aa6808463569916829670";
        let data = hex::decode(packet).unwrap();
        Message::decode(&data).unwrap();
    }

    #[test]
    fn encode_decode_enr_msg() {
        use alloy_rlp::Decodable;
        use enr::secp256k1::SecretKey;
        use std::net::Ipv4Addr;

        let mut rng = rand::rngs::OsRng;
        let key = SecretKey::new(&mut rng);
        let ip = Ipv4Addr::new(127, 0, 0, 1);
        let tcp = 3000;

        let fork_id: ForkId = ForkId { hash: ForkHash([220, 233, 108, 45]), next: 0u64 };

        let enr = {
            let mut builder = Enr::builder();
            builder.ip(ip.into());
            builder.tcp4(tcp);
            let mut buf = Vec::new();
            let forkentry = EnrForkIdEntry { fork_id };
            forkentry.encode(&mut buf);
            builder.add_value_rlp("eth", buf.into());
            builder.build(&key).unwrap()
        };

        let enr_response = EnrResponse { request_hash: rng.gen(), enr };

        let mut buf = Vec::new();
        enr_response.encode(&mut buf);

        let decoded = EnrResponse::decode(&mut &buf[..]).unwrap();

        let fork_id_decoded = decoded.eth_fork_id().unwrap();
        assert_eq!(fork_id, fork_id_decoded);
    }

    // test vector from the enr library rlp encoding tests
    // <https://github.com/sigp/enr/blob/e59dcb45ea07e423a7091d2a6ede4ad6d8ef2840/src/lib.rs#L1019>

    #[test]
    fn encode_known_rlp_enr() {
        use alloy_rlp::Decodable;
        use enr::{secp256k1::SecretKey, EnrPublicKey};
        use std::net::Ipv4Addr;

        let valid_record = hex!("f884b8407098ad865b00a582051940cb9cf36836572411a47278783077011599ed5cd16b76f2635f4e234738f30813a89eb9137e3e3df5266e3a1f11df72ecf1145ccb9c01826964827634826970847f00000189736563703235366b31a103ca634cae0d49acb401d8a4c6b6fe8c55b70d115bf400769cc1400f3258cd31388375647082765f");
        let signature = hex!("7098ad865b00a582051940cb9cf36836572411a47278783077011599ed5cd16b76f2635f4e234738f30813a89eb9137e3e3df5266e3a1f11df72ecf1145ccb9c");
        let expected_pubkey =
            hex!("03ca634cae0d49acb401d8a4c6b6fe8c55b70d115bf400769cc1400f3258cd3138");

        let enr = Enr::<SecretKey>::decode(&mut &valid_record[..]).unwrap();
        let pubkey = enr.public_key().encode();

        assert_eq!(enr.ip4(), Some(Ipv4Addr::new(127, 0, 0, 1)));
        assert_eq!(enr.id(), Some(String::from("v4")));
        assert_eq!(enr.udp4(), Some(DEFAULT_DISCOVERY_PORT));
        assert_eq!(enr.tcp4(), None);
        assert_eq!(enr.signature(), &signature[..]);
        assert_eq!(pubkey.to_vec(), expected_pubkey);
        assert!(enr.verify());

        assert_eq!(&alloy_rlp::encode(&enr)[..], &valid_record[..]);

        // ensure the length is equal
        assert_eq!(enr.length(), valid_record.len());
    }

    // test vector from the enr library rlp encoding tests
    // <https://github.com/sigp/enr/blob/e59dcb45ea07e423a7091d2a6ede4ad6d8ef2840/src/lib.rs#L1019>
    #[test]
    fn decode_enr_rlp() {
        use enr::secp256k1::SecretKey;
        use std::net::Ipv4Addr;

        let valid_record = hex!("f884b8407098ad865b00a582051940cb9cf36836572411a47278783077011599ed5cd16b76f2635f4e234738f30813a89eb9137e3e3df5266e3a1f11df72ecf1145ccb9c01826964827634826970847f00000189736563703235366b31a103ca634cae0d49acb401d8a4c6b6fe8c55b70d115bf400769cc1400f3258cd31388375647082765f");
        let signature = hex!("7098ad865b00a582051940cb9cf36836572411a47278783077011599ed5cd16b76f2635f4e234738f30813a89eb9137e3e3df5266e3a1f11df72ecf1145ccb9c");
        let expected_pubkey =
            hex!("03ca634cae0d49acb401d8a4c6b6fe8c55b70d115bf400769cc1400f3258cd3138");

        let mut valid_record_buf = valid_record.as_slice();
        let enr = Enr::<SecretKey>::decode(&mut valid_record_buf).unwrap();
        let pubkey = enr.public_key().encode();

        // Byte array must be consumed after enr has finished decoding
        assert!(valid_record_buf.is_empty());

        assert_eq!(enr.ip4(), Some(Ipv4Addr::new(127, 0, 0, 1)));
        assert_eq!(enr.id(), Some(String::from("v4")));
        assert_eq!(enr.udp4(), Some(DEFAULT_DISCOVERY_PORT));
        assert_eq!(enr.tcp4(), None);
        assert_eq!(enr.signature(), &signature[..]);
        assert_eq!(pubkey.to_vec(), expected_pubkey);
        assert!(enr.verify());
    }

    // test for failing message decode
    #[test]
    fn decode_failing_packet() {
        let packet = hex!("2467ab56952aedf4cfb8bb7830ddc8922d0f992185229919dad9de3841fe95d9b3a7b52459398235f6d3805644666d908b45edb3670414ed97f357afba51f71f7d35c1f45878ba732c3868b04ca42ff0ed347c99efcf3a5768afed68eb21ef960001db04c3808080c9840a480e8f82765f808466a9a06386019106833efe");

        let _message = Message::decode(&packet[..]).unwrap();
    }

    // test for failing message decode
    #[test]
    fn decode_node() {
        let packet = hex!("cb840000000082115c82115d");
        let _message = NodeEndpoint::decode(&mut &packet[..]).unwrap();
    }

    // test vector from the enr library rlp encoding tests
    // <https://github.com/sigp/enr/blob/e59dcb45ea07e423a7091d2a6ede4ad6d8ef2840/src/lib.rs#LL1206C35-L1206C35>
    #[test]
    fn encode_decode_enr_rlp() {
        use enr::{secp256k1::SecretKey, EnrPublicKey};
        use std::net::Ipv4Addr;

        let key = SecretKey::new(&mut rand::rngs::OsRng);
        let ip = Ipv4Addr::new(127, 0, 0, 1);
        let tcp = 3000;

        let enr = {
            let mut builder = Enr::builder();
            builder.ip(ip.into());
            builder.tcp4(tcp);
            builder.build(&key).unwrap()
        };

        let mut encoded_bytes = &alloy_rlp::encode(&enr)[..];
        let decoded_enr = Enr::<SecretKey>::decode(&mut encoded_bytes).unwrap();

        // Byte array must be consumed after enr has finished decoding
        assert!(encoded_bytes.is_empty());

        assert_eq!(decoded_enr, enr);
        assert_eq!(decoded_enr.id(), Some("v4".into()));
        assert_eq!(decoded_enr.ip4(), Some(ip));
        assert_eq!(decoded_enr.tcp4(), Some(tcp));
        assert_eq!(
            decoded_enr.public_key().encode(),
            key.public_key(secp256k1::SECP256K1).encode()
        );
        assert!(decoded_enr.verify());
    }

    mod eip8 {
        use super::*;

        fn junk_enr_request() -> Vec<u8> {
            let mut buf = Vec::new();
            // enr request is just an expiration
            let expire: u64 = 123456;

            // add some junk
            let junk: u64 = 112233;

            // rlp header encoding
            let payload_length = expire.length() + junk.length();
            alloy_rlp::Header { list: true, payload_length }.encode(&mut buf);

            // fields
            expire.encode(&mut buf);
            junk.encode(&mut buf);

            buf
        }

        // checks that junk data at the end of the packet is discarded according to eip-8
        #[test]
        fn eip8_decode_enr_request() {
            let enr_request_with_junk = junk_enr_request();

            let mut buf = enr_request_with_junk.as_slice();
            let decoded = EnrRequest::decode(&mut buf).unwrap();
            assert_eq!(decoded.expire, 123456);
        }

        // checks that junk data at the end of the packet is discarded according to eip-8
        //
        // test vector from eip-8: https://eips.ethereum.org/EIPS/eip-8
        #[test]
        fn eip8_decode_findnode() {
            let findnode_with_junk = hex!("c7c44041b9f7c7e41934417ebac9a8e1a4c6298f74553f2fcfdcae6ed6fe53163eb3d2b52e39fe91831b8a927bf4fc222c3902202027e5e9eb812195f95d20061ef5cd31d502e47ecb61183f74a504fe04c51e73df81f25c4d506b26db4517490103f84eb840ca634cae0d49acb401d8a4c6b6fe8c55b70d115bf400769cc1400f3258cd31387574077f301b421bc84df7266c44e9e6d569fc56be00812904767bf5ccd1fc7f8443b9a35582999983999999280dc62cc8255c73471e0a61da0c89acdc0e035e260add7fc0c04ad9ebf3919644c91cb247affc82b69bd2ca235c71eab8e49737c937a2c396");

            let buf = findnode_with_junk.as_slice();
            let decoded = Message::decode(buf).unwrap();

            let expected_id = hex!("ca634cae0d49acb401d8a4c6b6fe8c55b70d115bf400769cc1400f3258cd31387574077f301b421bc84df7266c44e9e6d569fc56be00812904767bf5ccd1fc7f");
            assert_matches!(decoded.msg, Message::FindNode(FindNode { id, expire: 1136239445 }) if id == expected_id);
        }

        // checks that junk data at the end of the packet is discarded according to eip-8
        //
        // test vector from eip-8: https://eips.ethereum.org/EIPS/eip-8
        #[test]
        fn eip8_decode_neighbours() {
            let neighbours_with_junk = hex!("c679fc8fe0b8b12f06577f2e802d34f6fa257e6137a995f6f4cbfc9ee50ed3710faf6e66f932c4c8d81d64343f429651328758b47d3dbc02c4042f0fff6946a50f4a49037a72bb550f3a7872363a83e1b9ee6469856c24eb4ef80b7535bcf99c0004f9015bf90150f84d846321163782115c82115db8403155e1427f85f10a5c9a7755877748041af1bcd8d474ec065eb33df57a97babf54bfd2103575fa829115d224c523596b401065a97f74010610fce76382c0bf32f84984010203040101b840312c55512422cf9b8a4097e9a6ad79402e87a15ae909a4bfefa22398f03d20951933beea1e4dfa6f968212385e829f04c2d314fc2d4e255e0d3bc08792b069dbf8599020010db83c4d001500000000abcdef12820d05820d05b84038643200b172dcfef857492156971f0e6aa2c538d8b74010f8e140811d53b98c765dd2d96126051913f44582e8c199ad7c6d6819e9a56483f637feaac9448aacf8599020010db885a308d313198a2e037073488203e78203e8b8408dcab8618c3253b558d459da53bd8fa68935a719aff8b811197101a4b2b47dd2d47295286fc00cc081bb542d760717d1bdd6bec2c37cd72eca367d6dd3b9df738443b9a355010203b525a138aa34383fec3d2719a0");

            let buf = neighbours_with_junk.as_slice();
            let decoded = Message::decode(buf).unwrap();

            let _ = NodeRecord {
                address: "99.33.22.55".parse().unwrap(),
                tcp_port: 4444,
                udp_port: 4445,
                id: hex!("3155e1427f85f10a5c9a7755877748041af1bcd8d474ec065eb33df57a97babf54bfd2103575fa829115d224c523596b401065a97f74010610fce76382c0bf32").into(),
            }.length();

            let expected_nodes: Vec<NodeRecord> = vec![
                NodeRecord {
                    address: "99.33.22.55".parse().unwrap(),
                    udp_port: 4444,
                    tcp_port: 4445,
                    id: hex!("3155e1427f85f10a5c9a7755877748041af1bcd8d474ec065eb33df57a97babf54bfd2103575fa829115d224c523596b401065a97f74010610fce76382c0bf32").into(),
                },
                NodeRecord {
                    address: "1.2.3.4".parse().unwrap(),
                    udp_port: 1,
                    tcp_port: 1,
                    id: hex!("312c55512422cf9b8a4097e9a6ad79402e87a15ae909a4bfefa22398f03d20951933beea1e4dfa6f968212385e829f04c2d314fc2d4e255e0d3bc08792b069db").into(),
                },
                NodeRecord {
                    address: "2001:db8:3c4d:15::abcd:ef12".parse().unwrap(),
                    udp_port: 3333,
                    tcp_port: 3333,
                    id: hex!("38643200b172dcfef857492156971f0e6aa2c538d8b74010f8e140811d53b98c765dd2d96126051913f44582e8c199ad7c6d6819e9a56483f637feaac9448aac").into(),
                },
                NodeRecord {
                    address: "2001:db8:85a3:8d3:1319:8a2e:370:7348".parse().unwrap(),
                    udp_port: 999,
                    tcp_port: 1000,
                    id: hex!("8dcab8618c3253b558d459da53bd8fa68935a719aff8b811197101a4b2b47dd2d47295286fc00cc081bb542d760717d1bdd6bec2c37cd72eca367d6dd3b9df73").into(),
                },
            ];
            assert_matches!(decoded.msg, Message::Neighbours(Neighbours { nodes, expire: 1136239445 }) if nodes == expected_nodes);
        }
    }
}
