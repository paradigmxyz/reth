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

#[derive(Clone, Copy, Debug, RlpEncodable, RlpDecodable)]
pub struct FindNodeMessage {
    pub id: NodeId,
    pub expire: u64,
}

#[derive(Clone, Debug, RlpEncodable, RlpDecodable)]
pub struct NeighboursMessage {
    pub nodes: Vec<NodeRecord>,
    pub expire: u64,
}

#[derive(Debug, Clone)]
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
