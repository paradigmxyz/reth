#![allow(missing_docs)]

use crate::{error::DecodePacketError, node::NodeRecord, NodeId, MAX_PACKET_SIZE, MIN_PACKET_SIZE};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use reth_primitives::{keccak256, H256};
use reth_rlp::{Decodable, DecodeError, Encodable, Header};
use reth_rlp_derive::{RlpDecodable, RlpEncodable};
use secp256k1::{
    ecdsa::{RecoverableSignature, RecoveryId},
    SecretKey, SECP256K1,
};
use std::{
    net::{IpAddr, Ipv6Addr, SocketAddr},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub type RequestId = u64;

pub const PING_TIMEOUT: Duration = Duration::from_secs(5);
pub const PING_INTERVAL: Duration = Duration::from_secs(10);
pub const FIND_NODE_TIMEOUT: Duration = Duration::from_secs(10);
pub const NEIGHBOURS_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
pub const NEIGHBOURS_EXPIRY_TIME: Duration = Duration::from_secs(30);

/// The size of the datagram is limited, so we chunk here the max number that fit in the datagram is
/// 12: (MAX_PACKET_SIZE - (header + expire + rlp overhead) / rlplength(NodeRecord). The unhappy
/// case is all IPV6 IPS
pub const MAX_DATAGRAM_NEIGHBOUR_RECORDS: usize = 12usize;

pub(crate) fn ping_expiry() -> u64 {
    (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + PING_TIMEOUT).as_secs()
}

pub(crate) fn find_node_expiry() -> u64 {
    (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + FIND_NODE_TIMEOUT).as_secs()
}

pub(crate) fn send_neighbours_expiry() -> u64 {
    (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + NEIGHBOURS_EXPIRY_TIME).as_secs()
}

// Note: this is adapted from https://github.com/vorot93/discv4

/// Id for message variants.
#[repr(u8)]
pub enum MessageId {
    Ping = 1,
    Pong = 2,
    FindNode = 3,
    Neighbours = 4,
}

impl MessageId {
    /// Converts the byte that represents the message id to the enum.
    fn from_u8(msg: u8) -> Result<Self, u8> {
        let msg = match msg {
            1 => MessageId::Ping,
            2 => MessageId::Pong,
            3 => MessageId::FindNode,
            4 => MessageId::Neighbours,
            _ => return Err(msg),
        };
        Ok(msg)
    }
}

/// All message variants
#[derive(Debug, Eq, PartialEq)]
pub enum Message {
    Ping(Ping),
    Pong(Pong),
    FindNode(FindNode),
    Neighbours(Neighbours),
}

// === impl Message ===

impl Message {
    /// Encodes the UDP datagram, See <https://github.com/ethereum/devp2p/blob/master/discv4.md#wire-protocol>
    ///
    /// The datagram is `header || payload`
    /// where header is `hash || signature || packet-type`
    pub fn encode(&self, secret_key: &SecretKey) -> (Bytes, H256) {
        // allocate max packet size
        let mut datagram = BytesMut::with_capacity(MAX_PACKET_SIZE);

        // since signature has fixed len, we can split and fill the datagram buffer at fixed
        // positions, this way we can encode the message directly in the datagram buffer
        let mut sig_bytes = datagram.split_off(H256::len_bytes());
        let mut payload = sig_bytes.split_off(secp256k1::constants::COMPACT_SIGNATURE_SIZE + 1);

        match self {
            Message::Ping(message) => {
                payload.put_u8(1);
                message.encode(&mut payload);
            }
            Message::Pong(message) => {
                payload.put_u8(2);
                message.encode(&mut payload);
            }
            Message::FindNode(message) => {
                payload.put_u8(3);
                message.encode(&mut payload);
            }
            Message::Neighbours(message) => {
                payload.put_u8(4);
                message.encode(&mut payload);
            }
        }

        let signature: RecoverableSignature = SECP256K1.sign_ecdsa_recoverable(
            &secp256k1::Message::from_slice(keccak256(&payload).as_ref())
                .expect("is correct MESSAGE_SIZE; qed"),
            secret_key,
        );

        let (rec, sig) = signature.serialize_compact();
        sig_bytes.extend_from_slice(&sig);
        sig_bytes.put_u8(rec.to_i32() as u8);
        sig_bytes.unsplit(payload);

        let hash = keccak256(&sig_bytes);
        datagram.extend_from_slice(hash.as_bytes());

        datagram.unsplit(sig_bytes);

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
        let data_hash = H256::from_slice(&packet[..32]);
        if data_hash != header_hash {
            return Err(DecodePacketError::HashMismatch)
        }

        let signature = &packet[32..96];
        let recovery_id = RecoveryId::from_i32(packet[96] as i32)?;
        let recoverable_sig = RecoverableSignature::from_compact(signature, recovery_id)?;

        // recover the public key
        let msg = secp256k1::Message::from_slice(keccak256(&packet[97..]).as_bytes())?;

        let pk = SECP256K1.recover_ecdsa(&msg, &recoverable_sig)?;
        let node_id = NodeId::from_slice(&pk.serialize_uncompressed()[1..]);

        let msg_type = packet[97];
        let payload = &mut &packet[98..];

        let msg = match MessageId::from_u8(msg_type).map_err(DecodePacketError::UnknownMessage)? {
            MessageId::Ping => Message::Ping(Ping::decode(payload)?),
            MessageId::Pong => Message::Pong(Pong::decode(payload)?),
            MessageId::FindNode => Message::FindNode(FindNode::decode(payload)?),
            MessageId::Neighbours => Message::Neighbours(Neighbours::decode(payload)?),
        };

        if !payload.is_empty() {
            return Err(DecodeError::UnexpectedLength.into())
        }

        Ok(Packet { msg, node_id, hash: header_hash })
    }
}

/// Decoded packet
#[derive(Debug)]
pub struct Packet {
    pub msg: Message,
    pub node_id: NodeId,
    pub hash: H256,
}

/// Represents the `from`, `to` fields in the packets
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeEndpoint {
    pub address: IpAddr,
    pub udp_port: u16,
    pub tcp_port: u16,
}

impl NodeEndpoint {
    /// The UDP socket address of this node
    #[must_use]
    pub fn udp_addr(&self) -> SocketAddr {
        SocketAddr::new(self.address, self.udp_port)
    }
}

impl Decodable for NodeEndpoint {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let Point { octets, udp_port, tcp_port } = Point::decode(buf)?;

        Ok(Self { address: octets.into(), udp_port, tcp_port })
    }
}

impl Encodable for NodeEndpoint {
    fn encode(&self, out: &mut dyn BufMut) {
        let octets = match self.address {
            IpAddr::V4(addr) => Octets::V4(addr.octets()),
            IpAddr::V6(addr) => Octets::V6(addr.octets()),
        };
        let p = Point { octets, udp_port: self.udp_port, tcp_port: self.tcp_port };
        p.encode(out)
    }
}

impl From<NodeRecord> for NodeEndpoint {
    fn from(NodeRecord { address, tcp_port, udp_port, .. }: NodeRecord) -> Self {
        Self { address, tcp_port, udp_port }
    }
}

/// A [FindNode packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#findnode-packet-0x03).).
#[derive(Clone, Copy, Debug, Eq, PartialEq, RlpEncodable, RlpDecodable)]
pub struct FindNode {
    pub id: NodeId,
    pub expire: u64,
}

/// A [Neighbours packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#neighbors-packet-0x04).
#[derive(Clone, Debug, Eq, PartialEq, RlpEncodable, RlpDecodable)]
pub struct Neighbours {
    pub nodes: Vec<NodeRecord>,
    pub expire: u64,
}

/// A [Ping packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#ping-packet-0x01).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Ping {
    pub from: NodeEndpoint,
    pub to: NodeEndpoint,
    pub expire: u64,
}

impl Encodable for Ping {
    fn encode(&self, out: &mut dyn BufMut) {
        #[derive(RlpEncodable)]
        struct V4PingMessage<'a> {
            pub version: u32,
            pub from: &'a NodeEndpoint,
            pub to: &'a NodeEndpoint,
            pub expire: u64,
        }
        V4PingMessage {
            version: 4, // version 4
            from: &self.from,
            to: &self.to,
            expire: self.expire,
        }
        .encode(out)
    }
}

impl Decodable for Ping {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        #[derive(RlpDecodable)]
        struct V4PingMessage {
            pub version: u32,
            pub from: NodeEndpoint,
            pub to: NodeEndpoint,
            pub expire: u64,
        }

        let ping = V4PingMessage::decode(buf)?;

        Ok(Ping { from: ping.from, to: ping.to, expire: ping.expire })
    }
}

/// A [Pong packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#pong-packet-0x02).
#[derive(Clone, Debug, Eq, PartialEq, RlpEncodable, RlpDecodable)]
pub struct Pong {
    pub to: NodeEndpoint,
    pub echo: H256,
    pub expire: u64,
}

/// Helper type for rlp codec
#[derive(RlpDecodable, RlpEncodable)]
struct Point {
    octets: Octets,
    udp_port: u16,
    tcp_port: u16,
}

/// IpAddr octets
pub(crate) enum Octets {
    V4([u8; 4]),
    V6([u8; 16]),
}

impl From<Octets> for IpAddr {
    fn from(value: Octets) -> Self {
        match value {
            Octets::V4(o) => IpAddr::from(o),
            Octets::V6(o) => {
                let ipv6 = Ipv6Addr::from(o);
                // If the ipv6 is ipv4 compatible/mapped, simply return the ipv4.
                if let Some(ipv4) = ipv6.to_ipv4() {
                    IpAddr::V4(ipv4)
                } else {
                    IpAddr::V6(ipv6)
                }
            }
        }
    }
}

impl Encodable for Octets {
    fn encode(&self, out: &mut dyn BufMut) {
        let octets = match self {
            Octets::V4(ref o) => &o[..],
            Octets::V6(ref o) => &o[..],
        };
        octets.encode(out)
    }
}

impl Decodable for Octets {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let h = Header::decode(buf)?;
        if h.list {
            return Err(DecodeError::UnexpectedList)
        }
        let o = match h.payload_length {
            4 => {
                let mut to = [0_u8; 4];
                to.copy_from_slice(&buf[..4]);
                Octets::V4(to)
            }
            16 => {
                let mut to = [0u8; 16];
                to.copy_from_slice(&buf[..16]);
                Octets::V6(to)
            }
            _ => return Err(DecodeError::UnexpectedLength),
        };
        buf.advance(h.payload_length);
        Ok(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use rand::{thread_rng, Rng, RngCore};

    fn rng_endpoint(rng: &mut impl Rng) -> NodeEndpoint {
        let address = if rng.gen() {
            let mut ip = [0u8; 4];
            rng.fill_bytes(&mut ip);
            IpAddr::V4(ip.into())
        } else {
            let mut ip = [0u8; 16];
            rng.fill_bytes(&mut ip);
            IpAddr::V6(ip.into())
        };
        NodeEndpoint { address, tcp_port: rng.gen(), udp_port: rng.gen() }
    }

    fn rng_record(rng: &mut impl RngCore) -> NodeRecord {
        let NodeEndpoint { address, udp_port, tcp_port } = rng_endpoint(rng);
        NodeRecord { address, tcp_port, udp_port, id: NodeId::random() }
    }

    fn rng_ipv6_record(rng: &mut impl RngCore) -> NodeRecord {
        let mut ip = [0u8; 16];
        rng.fill_bytes(&mut ip);
        let address = IpAddr::V6(ip.into());
        NodeRecord { address, tcp_port: rng.gen(), udp_port: rng.gen(), id: NodeId::random() }
    }

    fn rng_ipv4_record(rng: &mut impl RngCore) -> NodeRecord {
        let mut ip = [0u8; 4];
        rng.fill_bytes(&mut ip);
        let address = IpAddr::V4(ip.into());
        NodeRecord { address, tcp_port: rng.gen(), udp_port: rng.gen(), id: NodeId::random() }
    }

    fn rng_message(rng: &mut impl RngCore) -> Message {
        match rng.gen_range(1..=4) {
            1 => Message::Ping(Ping {
                from: rng_endpoint(rng),
                to: rng_endpoint(rng),
                expire: rng.gen(),
            }),
            2 => Message::Pong(Pong {
                to: rng_endpoint(rng),
                echo: H256::random(),
                expire: rng.gen(),
            }),
            3 => Message::FindNode(FindNode { id: NodeId::random(), expire: rng.gen() }),
            4 => {
                let num: usize = rng.gen_range(1..=MAX_DATAGRAM_NEIGHBOUR_RECORDS);
                Message::Neighbours(Neighbours {
                    nodes: std::iter::repeat_with(|| rng_record(rng)).take(num).collect(),
                    expire: rng.gen(),
                })
            }
            _ => unreachable!(),
        }
    }

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

            let mut buf = BytesMut::new();
            msg.encode(&mut buf);

            let decoded = NodeEndpoint::decode(&mut buf.as_ref()).unwrap();
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

            let mut buf = BytesMut::new();
            msg.encode(&mut buf);

            let decoded = NodeEndpoint::decode(&mut buf.as_ref()).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_ping_message() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 16];
            rng.fill_bytes(&mut ip);
            let msg = Ping { from: rng_endpoint(&mut rng), to: rng_endpoint(&mut rng), expire: 0 };

            let mut buf = BytesMut::new();
            msg.encode(&mut buf);

            let decoded = Ping::decode(&mut buf.as_ref()).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_hash_mismatch() {
        let mut rng = thread_rng();
        let msg = rng_message(&mut rng);
        let (secret_key, _) = SECP256K1.generate_keypair(&mut rng);
        let (buf, _) = msg.encode(&secret_key);
        let mut buf = BytesMut::from(buf.as_ref());
        buf.put_u8(0);
        match Message::decode(buf.as_ref()).unwrap_err() {
            DecodePacketError::HashMismatch => {}
            err => {
                unreachable!("unexpected err {}", err)
            }
        }
    }

    #[test]
    fn neighbours_max_nodes() {
        let mut rng = thread_rng();
        for _ in 0..1000 {
            let msg = Message::Neighbours(Neighbours {
                nodes: std::iter::repeat_with(|| rng_ipv6_record(&mut rng))
                    .take(MAX_DATAGRAM_NEIGHBOUR_RECORDS)
                    .collect(),
                expire: rng.gen(),
            });
            let (secret_key, _) = SECP256K1.generate_keypair(&mut rng);

            let (encoded, _) = msg.encode(&secret_key);
            assert!(encoded.len() <= MAX_PACKET_SIZE, "{} {:?}", encoded.len(), msg);

            let mut neighbours = Neighbours {
                nodes: std::iter::repeat_with(|| rng_ipv6_record(&mut rng))
                    .take(MAX_DATAGRAM_NEIGHBOUR_RECORDS - 1)
                    .collect(),
                expire: rng.gen(),
            };
            neighbours.nodes.push(rng_ipv4_record(&mut rng));
            let msg = Message::Neighbours(neighbours);
            let (encoded, _) = msg.encode(&secret_key);
            assert!(encoded.len() <= MAX_PACKET_SIZE, "{} {:?}", encoded.len(), msg);
        }
    }

    // tests that additional data is considered an error
    #[test]
    fn test_too_big_packet() {
        let mut rng = thread_rng();
        let msg = rng_message(&mut rng);
        let (secret_key, _) = SECP256K1.generate_keypair(&mut rng);

        let mut datagram = BytesMut::with_capacity(MAX_PACKET_SIZE);
        let mut sig_bytes = datagram.split_off(H256::len_bytes());
        let mut payload = sig_bytes.split_off(secp256k1::constants::COMPACT_SIGNATURE_SIZE + 1);

        match msg {
            Message::Ping(message) => {
                payload.put_u8(1);
                message.encode(&mut payload);
            }
            Message::Pong(message) => {
                payload.put_u8(2);
                message.encode(&mut payload);
            }
            Message::FindNode(message) => {
                payload.put_u8(3);
                message.encode(&mut payload);
            }
            Message::Neighbours(message) => {
                payload.put_u8(4);
                message.encode(&mut payload);
            }
        }
        payload.put_u8(42);

        let signature: RecoverableSignature = SECP256K1.sign_ecdsa_recoverable(
            &secp256k1::Message::from_slice(keccak256(&payload).as_ref())
                .expect("is correct MESSAGE_SIZE; qed"),
            &secret_key,
        );

        let (rec, sig) = signature.serialize_compact();
        sig_bytes.extend_from_slice(&sig);
        sig_bytes.put_u8(rec.to_i32() as u8);
        sig_bytes.unsplit(payload);

        let hash = keccak256(&sig_bytes);
        datagram.extend_from_slice(hash.as_bytes());

        datagram.unsplit(sig_bytes);

        match Message::decode(datagram.as_ref()).unwrap_err() {
            DecodePacketError::Rlp(DecodeError::UnexpectedLength) => {}
            err => {
                unreachable!("unexpected err {}", err)
            }
        }
    }

    #[test]
    fn test_encode_decode_message() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let msg = rng_message(&mut rng);
            let (secret_key, pk) = SECP256K1.generate_keypair(&mut rng);
            let sender_id = NodeId::from_slice(&pk.serialize_uncompressed()[1..]);

            let (buf, _) = msg.encode(&secret_key);

            let packet = Message::decode(buf.as_ref()).unwrap();

            assert_eq!(msg, packet.msg);
            assert_eq!(sender_id, packet.node_id);
        }
    }
}
