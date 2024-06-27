pub use reth_network_peers::{NodeRecord, NodeRecordParseError, TrustedPeer};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;


#[cfg(test)]
mod tests {
    use super::*;
    use alloy_rlp::Decodable;
    use rand::{thread_rng, Rng, RngCore};
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_mapped_ipv6() {
        let mut rng = thread_rng();

        let v4: Ipv4Addr = "0.0.0.0".parse().unwrap();
        let v6 = v4.to_ipv6_mapped();

        let record = NodeRecord {
            address: v6.into(),
            tcp_port: rng.gen(),
            udp_port: rng.gen(),
            id: rng.gen(),
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
            id: rng.gen(),
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
                id: rng.gen(),
            };

            let decoded = NodeRecord::decode(&mut alloy_rlp::encode(record).as_slice()).unwrap();
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
                id: rng.gen(),
            };

            let decoded = NodeRecord::decode(&mut alloy_rlp::encode(record).as_slice()).unwrap();
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
            id: "6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0".parse().unwrap(),
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
            id: "6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0".parse().unwrap(),
        })
    }
}
