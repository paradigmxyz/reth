use crate::{MAX_PACKET_SIZE, node::NodeRecord, NodeId};
use bytes::{Buf, BufMut, BytesMut};
use reth_rlp::{Decodable, DecodeError, Encodable, Header};
use reth_rlp_derive::{RlpDecodable, RlpEncodable};
use std::net::{IpAddr, Ipv6Addr};
use reth_primitives::{H256, keccak256};
use secp256k1::{
    ecdsa::{RecoverableSignature}, SecretKey, SECP256K1,
};

// Note: this is adapted from https://github.com/vorot93/discv4

/// Id for message variants.
#[repr(u8)]
pub enum MessageId {
    Ping = 1,
    Pong = 2,
    FindNode = 3,
    Neighbours = 4,
}

/// All message variants
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
    pub fn encode(&self, secret_key: &SecretKey) -> BytesMut {

        // allocate max packet size
        let mut datagram = BytesMut::with_capacity(MAX_PACKET_SIZE);

        // since signature has fixed len, we can split and fill the datagram buffer at fixed positions, this way we can encode the message directly in the datagram buffer
        let mut sig_bytes = datagram.split_off(H256::len_bytes());
        let mut payload =
            sig_bytes.split_off(secp256k1::constants::COMPACT_SIGNATURE_SIZE + 1);

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
            &secp256k1::Message::from_slice(keccak256(&payload).as_ref()).expect("is correct MESSAGE_SIZE; qed"),
            secret_key,
        );

        let (rec, sig) = signature.serialize_compact();
        sig_bytes.extend_from_slice(&sig);
        sig_bytes.put_u8(rec.to_i32() as u8);
        sig_bytes.unsplit(payload);

        let hash = keccak256(&sig_bytes);
        datagram.extend_from_slice(hash.as_bytes());

        datagram.unsplit(sig_bytes);

        datagram
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub address: IpAddr,
    pub udp_port: u16,
    pub tcp_port: u16,
}

impl Decodable for Endpoint {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let Point { octets, udp_port, tcp_port } = Point::decode(buf)?;

        Ok(Self { address: octets.into(), udp_port, tcp_port })
    }
}

impl Encodable for Endpoint {
    fn encode(&self, out: &mut dyn BufMut) {
        let octets = match self.address {
            IpAddr::V4(addr) => Octets::V4(addr.octets()),
            IpAddr::V6(addr) => Octets::V6(addr.octets()),
        };
        let p = Point { octets, udp_port: self.udp_port, tcp_port: self.tcp_port };
        p.encode(out)
    }
}

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

impl From<NodeRecord> for Endpoint {
    fn from(NodeRecord { address, tcp_port, udp_port, .. }: NodeRecord) -> Self {
        Self { address, tcp_port, udp_port }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, RlpEncodable, RlpDecodable)]
pub struct FindNode {
    pub id: NodeId,
    pub expire: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, RlpEncodable, RlpDecodable)]
pub struct Neighbours {
    pub nodes: Vec<NodeRecord>,
    pub expire: u64,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Ping {
    pub from: Endpoint,
    pub to: Endpoint,
    pub expire: u64,
}

impl Encodable for Ping {
    fn encode(&self, out: &mut dyn BufMut) {
        #[derive(RlpEncodable)]
        struct V4PingMessage<'a> {
            pub version: u32,
            pub from: &'a Endpoint,
            pub to: &'a Endpoint,
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
            pub from: Endpoint,
            pub to: Endpoint,
            pub expire: u64,
        }

        let ping = V4PingMessage::decode(buf)?;

        Ok(Ping { from: ping.from, to: ping.to, expire: ping.expire })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, RlpEncodable, RlpDecodable)]
pub struct Pong {
    pub to: Endpoint,
    pub echo: H256,
    pub expire: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use rand::{thread_rng, Rng, RngCore};

    fn rng_endpoint(rng: &mut impl Rng) -> Endpoint {
        let address = if rng.gen() {
            let mut ip = [0u8; 4];
            rng.fill_bytes(&mut ip);
            IpAddr::V4(ip.into())
        } else {
            let mut ip = [0u8; 16];
            rng.fill_bytes(&mut ip);
            IpAddr::V6(ip.into())
        };
        Endpoint { address, tcp_port: rng.gen(), udp_port: rng.gen() }
    }

    #[test]
    fn test_endpoint_ipv_v4() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 4];
            rng.fill_bytes(&mut ip);
            let msg = Endpoint {
                address: IpAddr::V4(ip.into()),
                tcp_port: rng.gen(),
                udp_port: rng.gen(),
            };

            let mut buf = BytesMut::new();
            msg.encode(&mut buf);

            let decoded = Endpoint::decode(&mut buf.as_ref()).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_endpoint_ipv_64() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 16];
            rng.fill_bytes(&mut ip);
            let msg = Endpoint {
                address: IpAddr::V6(ip.into()),
                tcp_port: rng.gen(),
                udp_port: rng.gen(),
            };

            let mut buf = BytesMut::new();
            msg.encode(&mut buf);

            let decoded = Endpoint::decode(&mut buf.as_ref()).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_ping_message() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 16];
            rng.fill_bytes(&mut ip);
            let msg =
                Ping { from: rng_endpoint(&mut rng), to: rng_endpoint(&mut rng), expire: 0 };

            let mut buf = BytesMut::new();
            msg.encode(&mut buf);

            let decoded = Ping::decode(&mut buf.as_ref()).unwrap();
            assert_eq!(msg, decoded);
        }
    }
}
