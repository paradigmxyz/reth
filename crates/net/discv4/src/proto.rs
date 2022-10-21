use crate::{node::NodeRecord, NodeId};
use bytes::{Buf, BufMut};
use reth_rlp::{Decodable, DecodeError, Encodable, Header};
use reth_rlp_derive::{RlpDecodable, RlpEncodable};
use std::net::{IpAddr, Ipv6Addr};

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
pub struct FindNodeMessage {
    pub id: NodeId,
    pub expire: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, RlpEncodable, RlpDecodable)]
pub struct NeighboursMessage {
    pub nodes: Vec<NodeRecord>,
    pub expire: u64,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PingMessage {
    pub from: Endpoint,
    pub to: Endpoint,
    pub expire: u64,
}

impl Encodable for PingMessage {
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

impl Decodable for PingMessage {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        #[derive(RlpDecodable)]
        struct V4PingMessage {
            pub version: u32,
            pub from: Endpoint,
            pub to: Endpoint,
            pub expire: u64,
        }

        let ping = V4PingMessage::decode(buf)?;

        Ok(PingMessage { from: ping.from, to: ping.to, expire: ping.expire })
    }
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
                PingMessage { from: rng_endpoint(&mut rng), to: rng_endpoint(&mut rng), expire: 0 };

            let mut buf = BytesMut::new();
            msg.encode(&mut buf);

            let decoded = PingMessage::decode(&mut buf.as_ref()).unwrap();
            assert_eq!(msg, decoded);
        }
    }
}
