#![allow(missing_docs)]

use crate::{error::DecodePacketError, node::NodeRecord, PeerId, MAX_PACKET_SIZE, MIN_PACKET_SIZE};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use enr::Enr;
use reth_primitives::{keccak256, ForkId, H256};
use reth_rlp::{Decodable, DecodeError, Encodable, Header};
use reth_rlp_derive::{RlpDecodable, RlpEncodable};
use secp256k1::{
    ecdsa::{RecoverableSignature, RecoveryId},
    SecretKey, SECP256K1,
};
use std::net::{IpAddr, Ipv6Addr};

// Note: this is adapted from https://github.com/vorot93/discv4

/// Id for message variants.
#[derive(Debug)]
#[repr(u8)]
pub enum MessageId {
    Ping = 1,
    Pong = 2,
    FindNode = 3,
    Neighbours = 4,
    EnrRequest = 5,
    EnrResponse = 6,
}

impl MessageId {
    /// Converts the byte that represents the message id to the enum.
    fn from_u8(msg: u8) -> Result<Self, u8> {
        let msg = match msg {
            1 => MessageId::Ping,
            2 => MessageId::Pong,
            3 => MessageId::FindNode,
            4 => MessageId::Neighbours,
            5 => MessageId::EnrRequest,
            6 => MessageId::EnrResponse,
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
    EnrRequest(EnrRequest),
    EnrResponse(EnrResponse),
}

// === impl Message ===

impl Message {
    /// Returns the id for this type
    pub fn msg_type(&self) -> MessageId {
        match self {
            Message::Ping(_) => MessageId::Ping,
            Message::Pong(_) => MessageId::Pong,
            Message::FindNode(_) => MessageId::FindNode,
            Message::Neighbours(_) => MessageId::Neighbours,
            Message::EnrRequest(_) => MessageId::EnrRequest,
            Message::EnrResponse(_) => MessageId::EnrResponse,
        }
    }

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
            Message::EnrRequest(message) => {
                payload.put_u8(5);
                message.encode(&mut payload);
            }
            Message::EnrResponse(message) => {
                payload.put_u8(6);
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
        let node_id = PeerId::from_slice(&pk.serialize_uncompressed()[1..]);

        let msg_type = packet[97];
        let payload = &mut &packet[98..];

        let msg = match MessageId::from_u8(msg_type).map_err(DecodePacketError::UnknownMessage)? {
            MessageId::Ping => Message::Ping(Ping::decode(payload)?),
            MessageId::Pong => Message::Pong(Pong::decode(payload)?),
            MessageId::FindNode => Message::FindNode(FindNode::decode(payload)?),
            MessageId::Neighbours => Message::Neighbours(Neighbours::decode(payload)?),
            MessageId::EnrRequest => Message::EnrRequest(EnrRequest::decode(payload)?),
            MessageId::EnrResponse => Message::EnrResponse(EnrResponse::decode(payload)?),
        };

        Ok(Packet { msg, node_id, hash: header_hash })
    }
}

/// Decoded packet
#[derive(Debug)]
pub struct Packet {
    pub msg: Message,
    pub node_id: PeerId,
    pub hash: H256,
}

/// Represents the `from`, `to` fields in the packets
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeEndpoint {
    pub address: IpAddr,
    pub udp_port: u16,
    pub tcp_port: u16,
}
impl Decodable for NodeEndpoint {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let b = &mut &**buf;
        let rlp_head = Header::decode(b)?;
        if !rlp_head.list {
            return Err(DecodeError::UnexpectedString)
        }
        let started_len = b.len();
        let octets = Octets::decode(b)?;
        let this = Self {
            address: octets.into(),
            udp_port: Decodable::decode(b)?,
            tcp_port: Decodable::decode(b)?,
        };
        // the ENR record can contain additional entries that we skip
        let consumed = started_len - b.len();
        if consumed > rlp_head.payload_length {
            return Err(DecodeError::ListLengthMismatch {
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

impl Encodable for NodeEndpoint {
    fn encode(&self, out: &mut dyn BufMut) {
        #[derive(RlpEncodable)]
        struct RlpEndpoint {
            octets: Octets,
            udp_port: u16,
            tcp_port: u16,
        }

        let octets = match self.address {
            IpAddr::V4(addr) => Octets::V4(addr.octets()),
            IpAddr::V6(addr) => Octets::V6(addr.octets()),
        };
        let p = RlpEndpoint { octets, udp_port: self.udp_port, tcp_port: self.tcp_port };
        p.encode(out)
    }
}

impl From<NodeRecord> for NodeEndpoint {
    fn from(NodeRecord { address, tcp_port, udp_port, .. }: NodeRecord) -> Self {
        Self { address, tcp_port, udp_port }
    }
}

/// A [FindNode packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#findnode-packet-0x03).
#[derive(Clone, Copy, Debug, Eq, PartialEq, RlpEncodable, RlpDecodable)]
pub struct FindNode {
    pub id: PeerId,
    pub expire: u64,
}

/// A [Neighbours packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#neighbors-packet-0x04).
#[derive(Clone, Debug, Eq, PartialEq, RlpEncodable, RlpDecodable)]
pub struct Neighbours {
    pub nodes: Vec<NodeRecord>,
    pub expire: u64,
}

/// A [ENRRequest packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#enrrequest-packet-0x05).
#[derive(Clone, Copy, Debug, Eq, PartialEq, RlpEncodable, RlpDecodable)]
pub struct EnrRequest {
    pub expire: u64,
}

/// A [ENRResponse packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#enrresponse-packet-0x06).
#[derive(Clone, Debug, Eq, PartialEq, RlpEncodable)]
pub struct EnrResponse {
    pub request_hash: H256,
    pub enr: Enr<SecretKey>,
}

// === impl EnrResponse ===

impl EnrResponse {
    /// Returns the [`ForkId`] if set
    ///
    /// See also <https://github.com/ethereum/go-ethereum/blob/9244d5cd61f3ea5a7645fdf2a1a96d53421e412f/eth/protocols/eth/discovery.go#L36>
    pub fn eth_fork_id(&self) -> Option<ForkId> {
        let mut maybe_fork_id = self.enr.get(b"eth")?;
        ForkId::decode(&mut maybe_fork_id).ok()
    }
}

impl Decodable for EnrResponse {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let b = &mut &**buf;
        let rlp_head = Header::decode(b)?;
        if !rlp_head.list {
            return Err(DecodeError::UnexpectedString)
        }
        // let started_len = b.len();
        let this = Self {
            request_hash: reth_rlp::Decodable::decode(b)?,
            enr: reth_rlp::Decodable::decode(b)?,
        };
        // TODO: `Decodable` can be derived once we have native reth_rlp decoding for ENR: <https://github.com/paradigmxyz/reth/issues/482>
        // Skipping the size check here is fine since the `buf` is the UDP datagram
        // let consumed = started_len - b.len();
        // if consumed != rlp_head.payload_length {
        //     return Err(reth_rlp::DecodeError::ListLengthMismatch {
        //         expected: rlp_head.payload_length,
        //         got: consumed,
        //     })
        // }
        *buf = *b;
        Ok(this)
    }
}

/// A [Ping packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#ping-packet-0x01).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Ping {
    pub from: NodeEndpoint,
    pub to: NodeEndpoint,
    pub expire: u64,
    /// Optional enr_seq for <https://eips.ethereum.org/EIPS/eip-868>
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
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let b = &mut &**buf;
        let rlp_head = Header::decode(b)?;
        if !rlp_head.list {
            return Err(DecodeError::UnexpectedString)
        }
        let started_len = b.len();
        let _version = u32::decode(b)?;
        let mut this = Self {
            from: Decodable::decode(b)?,
            to: Decodable::decode(b)?,
            expire: Decodable::decode(b)?,
            enr_sq: None,
        };

        // only decode the ENR sequence if there's more data in the datagram to decode else skip
        if b.has_remaining() {
            this.enr_sq = Some(Decodable::decode(b)?);
        }

        let consumed = started_len - b.len();
        if consumed > rlp_head.payload_length {
            return Err(DecodeError::ListLengthMismatch {
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

/// A [Pong packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#pong-packet-0x02).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pong {
    pub to: NodeEndpoint,
    pub echo: H256,
    pub expire: u64,
    /// Optional enr_seq for <https://eips.ethereum.org/EIPS/eip-868>
    pub enr_sq: Option<u64>,
}

impl Encodable for Pong {
    fn encode(&self, out: &mut dyn BufMut) {
        #[derive(RlpEncodable)]
        struct PongMessageEIP868<'a> {
            to: &'a NodeEndpoint,
            echo: &'a H256,
            expire: u64,
            enr_seq: u64,
        }

        #[derive(RlpEncodable)]
        struct PongMessage<'a> {
            to: &'a NodeEndpoint,
            echo: &'a H256,
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
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let b = &mut &**buf;
        let rlp_head = Header::decode(b)?;
        if !rlp_head.list {
            return Err(DecodeError::UnexpectedString)
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
            return Err(DecodeError::ListLengthMismatch {
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

/// IpAddr octets
#[derive(Debug, Copy, Clone)]
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
    use crate::{
        mock::{rng_endpoint, rng_ipv4_record, rng_ipv6_record, rng_message},
        SAFE_MAX_DATAGRAM_NEIGHBOUR_RECORDS,
    };
    use bytes::BytesMut;
    use rand::{thread_rng, Rng, RngCore};

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
            let msg = Ping {
                from: rng_endpoint(&mut rng),
                to: rng_endpoint(&mut rng),
                expire: 0,
                enr_sq: None,
            };

            let mut buf = BytesMut::new();
            msg.encode(&mut buf);

            let decoded = Ping::decode(&mut buf.as_ref()).unwrap();
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

            let mut buf = BytesMut::new();
            msg.encode(&mut buf);

            let decoded = Ping::decode(&mut buf.as_ref()).unwrap();
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
                echo: H256::random(),
                expire: rng.gen(),
                enr_sq: None,
            };

            let mut buf = BytesMut::new();
            msg.encode(&mut buf);

            let decoded = Pong::decode(&mut buf.as_ref()).unwrap();
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
                echo: H256::random(),
                expire: rng.gen(),
                enr_sq: Some(rng.gen()),
            };

            let mut buf = BytesMut::new();
            msg.encode(&mut buf);

            let decoded = Pong::decode(&mut buf.as_ref()).unwrap();
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
            let sender_id = PeerId::from_slice(&pk.serialize_uncompressed()[1..]);

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
}
