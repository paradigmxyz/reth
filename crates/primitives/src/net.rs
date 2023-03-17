use crate::PeerId;
use reth_rlp::RlpDecodable;
use reth_rlp_derive::RlpEncodable;
use secp256k1::{SecretKey, SECP256K1};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::{
    fmt,
    fmt::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::ParseIntError,
    str::FromStr,
};
use url::{Host, Url};

/// Represents a ENR in discv4.
///
/// Note: this is only an excerpt of the [`NodeRecord`] data structure.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    PartialEq,
    Hash,
    SerializeDisplay,
    DeserializeFromStr,
    RlpEncodable,
    RlpDecodable,
)]
pub struct NodeRecord {
    /// The Address of a node.
    pub address: IpAddr,
    /// TCP port of the port that accepts connections.
    pub tcp_port: u16,
    /// UDP discovery port.
    pub udp_port: u16,
    /// Public key of the discovery service
    pub id: PeerId,
}

impl NodeRecord {
    /// Derive the [`NodeRecord`] from the secret key and addr
    pub fn from_secret_key(addr: SocketAddr, sk: &SecretKey) -> Self {
        let pk = secp256k1::PublicKey::from_secret_key(SECP256K1, sk);
        let id = PeerId::from_slice(&pk.serialize_uncompressed()[1..]);
        Self::new(addr, id)
    }

    /// Converts the `address` into an [`Ipv4Addr`] if the `address` is a mapped
    /// [Ipv6Addr](std::net::Ipv6Addr).
    ///
    /// Returns `true` if the address was converted.
    ///
    /// See also [std::net::Ipv6Addr::to_ipv4_mapped]
    pub fn convert_ipv4_mapped(&mut self) -> bool {
        // convert IPv4 mapped IPv6 address
        if let IpAddr::V6(v6) = self.address {
            if let Some(v4) = v6.to_ipv4_mapped() {
                self.address = v4.into();
                return true
            }
        }
        false
    }

    /// Same as [Self::convert_ipv4_mapped] but consumes the type
    pub fn into_ipv4_mapped(mut self) -> Self {
        self.convert_ipv4_mapped();
        self
    }

    /// Creates a new record from a socket addr and peer id.
    #[allow(unused)]
    pub fn new(addr: SocketAddr, id: PeerId) -> Self {
        Self { address: addr.ip(), tcp_port: addr.port(), udp_port: addr.port(), id }
    }

    /// The TCP socket address of this node
    #[must_use]
    pub fn tcp_addr(&self) -> SocketAddr {
        SocketAddr::new(self.address, self.tcp_port)
    }

    /// The UDP socket address of this node
    #[must_use]
    pub fn udp_addr(&self) -> SocketAddr {
        SocketAddr::new(self.address, self.udp_port)
    }
}

impl fmt::Display for NodeRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("enode://")?;
        hex::encode(self.id.as_bytes()).fmt(f)?;
        f.write_char('@')?;
        match self.address {
            IpAddr::V4(ip) => {
                ip.fmt(f)?;
            }
            IpAddr::V6(ip) => {
                // encapsulate with brackets
                f.write_char('[')?;
                ip.fmt(f)?;
                f.write_char(']')?;
            }
        }
        f.write_char(':')?;
        self.tcp_port.fmt(f)?;
        if self.tcp_port != self.udp_port {
            f.write_str("?discport=")?;
            self.udp_port.fmt(f)?;
        }

        Ok(())
    }
}

// <https://github.com/ledgerwatch/erigon/blob/610e648dc43ec8cd6563313e28f06f534a9091b3/params/bootnodes.go>

/// Ethereum Foundation Go Bootnodes
pub static MAINNET_BOOTNODES : [&str; 4] = [
    "enode://d860a01f9722d78051619d1e2351aba3f43f943f6f00718d1b9baa4101932a1f5011f16bb2b1bb35db20d6fe28fa0bf09636d26a87d31de9ec6203eeedb1f666@18.138.108.67:30303",   // bootnode-aws-ap-southeast-1-001
    "enode://22a8232c3abc76a16ae9d6c3b164f98775fe226f0917b0ca871128a74a8e9630b458460865bab457221f1d448dd9791d24c4e5d88786180ac185df813a68d4de@3.209.45.79:30303",     // bootnode-aws-us-east-1-001
    "enode://2b252ab6a1d0f971d9722cb839a42cb81db019ba44c08754628ab4a823487071b5695317c8ccd085219c3a03af063495b2f1da8d18218da2d6a82981b45e6ffc@65.108.70.101:30303",   // bootnode-hetzner-hel
    "enode://4aeb4ab6c14b23e2c4cfdce879c04b0748a20d8e9b59e25ded2a08143e265c6c25936e74cbc8e641e3312ca288673d91f2f93f8e277de3cfa444ecdaaf982052@157.90.35.166:30303",   // bootnode-hetzner-fsn
];

/// SEPOLIA BOOTNODES
pub static SEPOLIA_BOOTNODES : [&str; 2] = [
    // geth
    "enode://9246d00bc8fd1742e5ad2428b80fc4dc45d786283e05ef6edbd9002cbc335d40998444732fbe921cb88e1d2c73d1b1de53bae6a2237996e9bfe14f871baf7066@18.168.182.86:30303",
    // besu
    "enode://ec66ddcf1a974950bd4c782789a7e04f8aa7110a72569b6e65fcd51e937e74eed303b1ea734e4d19cfaec9fbff9b6ee65bf31dcb50ba79acce9dd63a6aca61c7@52.14.151.177:30303",
];

/// GOERLI bootnodes
pub static GOERLI_BOOTNODES : [&str; 7] = [
    // Upstream bootnodes
    "enode://011f758e6552d105183b1761c5e2dea0111bc20fd5f6422bc7f91e0fabbec9a6595caf6239b37feb773dddd3f87240d99d859431891e4a642cf2a0a9e6cbb98a@51.141.78.53:30303",
    "enode://176b9417f511d05b6b2cf3e34b756cf0a7096b3094572a8f6ef4cdcb9d1f9d00683bf0f83347eebdf3b81c3521c2332086d9592802230bf528eaf606a1d9677b@13.93.54.137:30303",
    "enode://46add44b9f13965f7b9875ac6b85f016f341012d84f975377573800a863526f4da19ae2c620ec73d11591fa9510e992ecc03ad0751f53cc02f7c7ed6d55c7291@94.237.54.114:30313",
    "enode://b5948a2d3e9d486c4d75bf32713221c2bd6cf86463302339299bd227dc2e276cd5a1c7ca4f43a0e9122fe9af884efed563bd2a1fd28661f3b5f5ad7bf1de5949@18.218.250.66:30303",

    // Ethereum Foundation bootnode
    "enode://a61215641fb8714a373c80edbfa0ea8878243193f57c96eeb44d0bc019ef295abd4e044fd619bfc4c59731a73fb79afe84e9ab6da0c743ceb479cbb6d263fa91@3.11.147.67:30303",

    // Goerli Initiative bootnodes
    "enode://d4f764a48ec2a8ecf883735776fdefe0a3949eb0ca476bd7bc8d0954a9defe8fea15ae5da7d40b5d2d59ce9524a99daedadf6da6283fca492cc80b53689fb3b3@46.4.99.122:32109",
    "enode://d2b720352e8216c9efc470091aa91ddafc53e222b32780f505c817ceef69e01d5b0b0797b69db254c586f493872352f5a022b4d8479a00fc92ec55f9ad46a27e@88.99.70.182:30303",
];

/// Returns parsed mainnet nodes
pub fn mainnet_nodes() -> Vec<NodeRecord> {
    parse_nodes(&MAINNET_BOOTNODES[..])
}

/// Returns parsed goerli nodes
pub fn goerli_nodes() -> Vec<NodeRecord> {
    parse_nodes(&GOERLI_BOOTNODES[..])
}

/// Returns parsed sepolia nodes
pub fn sepolia_nodes() -> Vec<NodeRecord> {
    parse_nodes(&SEPOLIA_BOOTNODES[..])
}

/// Parses all the nodes
fn parse_nodes(nodes: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<NodeRecord> {
    nodes.into_iter().map(|s| s.as_ref().parse().unwrap()).collect()
}

/// Possible error types when parsing a `NodeRecord`
#[derive(Debug, thiserror::Error)]
pub enum NodeRecordParseError {
    #[error("Failed to parse url: {0}")]
    InvalidUrl(String),
    #[error("Failed to parse id")]
    InvalidId(String),
    #[error("Failed to discport query: {0}")]
    Discport(ParseIntError),
}

impl FromStr for NodeRecord {
    type Err = NodeRecordParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url = Url::parse(s).map_err(|e| NodeRecordParseError::InvalidUrl(e.to_string()))?;

        let address = match url.host() {
            Some(Host::Ipv4(ip)) => IpAddr::V4(ip),
            Some(Host::Ipv6(ip)) => IpAddr::V6(ip),
            Some(Host::Domain(ip)) => IpAddr::V4(
                Ipv4Addr::from_str(ip)
                    .map_err(|e| NodeRecordParseError::InvalidUrl(e.to_string()))?,
            ),
            _ => return Err(NodeRecordParseError::InvalidUrl(format!("invalid host: {url:?}"))),
        };
        let port = url
            .port()
            .ok_or_else(|| NodeRecordParseError::InvalidUrl("no port specified".to_string()))?;

        let udp_port = if let Some(discovery_port) =
            url.query_pairs().find_map(|(maybe_disc, port)| {
                if maybe_disc.as_ref() == "discport" {
                    Some(port)
                } else {
                    None
                }
            }) {
            discovery_port.parse::<u16>().map_err(NodeRecordParseError::Discport)?
        } else {
            port
        };

        let id = url
            .username()
            .parse::<PeerId>()
            .map_err(|e| NodeRecordParseError::InvalidId(e.to_string()))?;

        Ok(Self { address, id, tcp_port: port, udp_port })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use rand::{thread_rng, Rng, RngCore};
    use reth_rlp::{Decodable, Encodable};

    #[test]
    fn test_mapped_ipv6() {
        let mut rng = thread_rng();

        let v4: Ipv4Addr = "0.0.0.0".parse().unwrap();
        let v6 = v4.to_ipv6_mapped();

        let record = NodeRecord {
            address: v6.into(),
            tcp_port: rng.gen(),
            udp_port: rng.gen(),
            id: PeerId::random(),
        };

        assert!(record.clone().convert_ipv4_mapped());
        assert_eq!(record.into_ipv4_mapped().address, IpAddr::from(v4));
    }

    #[test]
    fn test_mapped_ipv4() {
        let mut rng = thread_rng();
        let v4: Ipv4Addr = "0.0.0.0".parse().unwrap();

        let record = NodeRecord {
            address: v4.into(),
            tcp_port: rng.gen(),
            udp_port: rng.gen(),
            id: PeerId::random(),
        };

        assert!(!record.clone().convert_ipv4_mapped());
        assert_eq!(record.into_ipv4_mapped().address, IpAddr::from(v4));
    }

    #[test]
    fn test_noderecord_codec_ipv4() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 4];
            rng.fill_bytes(&mut ip);
            let record = NodeRecord {
                address: IpAddr::V4(ip.into()),
                tcp_port: rng.gen(),
                udp_port: rng.gen(),
                id: PeerId::random(),
            };

            let mut buf = BytesMut::new();
            record.encode(&mut buf);

            let decoded = NodeRecord::decode(&mut buf.as_ref()).unwrap();
            assert_eq!(record, decoded);
        }
    }

    #[test]
    fn test_noderecord_codec_ipv6() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 16];
            rng.fill_bytes(&mut ip);
            let record = NodeRecord {
                address: IpAddr::V6(ip.into()),
                tcp_port: rng.gen(),
                udp_port: rng.gen(),
                id: PeerId::random(),
            };

            let mut buf = BytesMut::new();
            record.encode(&mut buf);

            let decoded = NodeRecord::decode(&mut buf.as_ref()).unwrap();
            assert_eq!(record, decoded);
        }
    }

    #[test]
    fn test_url_parse() {
        let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301";
        let node: NodeRecord = url.parse().unwrap();
        assert_eq!(node, NodeRecord {
            address: IpAddr::V4([10,3,58,6].into()),
            tcp_port: 30303,
            udp_port: 30301,
            id: "6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0".parse().unwrap(),
        })
    }

    #[test]
    fn test_node_display() {
        let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303";
        let node: NodeRecord = url.parse().unwrap();
        assert_eq!(url, &format!("{node}"));
    }

    #[test]
    fn test_node_display_discport() {
        let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301";
        let node: NodeRecord = url.parse().unwrap();
        assert_eq!(url, &format!("{node}"));
    }

    #[test]
    fn test_node_serialize() {
        let node = NodeRecord{
            address: IpAddr::V4([10, 3, 58, 6].into()),
            tcp_port: 30303u16,
            udp_port: 30301u16,
            id: PeerId::from_str("6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0").unwrap(),
        };
        let ser = serde_json::to_string::<NodeRecord>(&node).expect("couldn't serialize");
        assert_eq!(ser, "\"enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301\"")
    }

    #[test]
    fn test_node_deserialize() {
        let url = "\"enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301\"";
        let node: NodeRecord = serde_json::from_str(url).expect("couldn't deserialize");
        assert_eq!(node, NodeRecord{
            address: IpAddr::V4([10, 3, 58, 6].into()),
            tcp_port: 30303u16,
            udp_port: 30301u16,
            id: PeerId::from_str("6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0").unwrap(),
        })
    }
}
