//! Interface between node identification on protocol version 5 and 4. Specifically, between types
//! [`enr::NodeId`] and [`PeerId`].

use enr::{CombinedPublicKey, Enr, EnrPublicKey, NodeId};
use reth_network_peers::{id2pk, pk2id, PeerId};
use secp256k1::{PublicKey, SecretKey};

/// Extracts a [`CombinedPublicKey::Secp256k1`] from a [`discv5::Enr`] and converts it to a
/// [`PeerId`]. Note: conversion from discv5 ID to discv4 ID is not possible.
pub fn enr_to_discv4_id(enr: &discv5::Enr) -> Option<PeerId> {
    let pk = enr.public_key();
    if !matches!(pk, CombinedPublicKey::Secp256k1(_)) {
        return None
    }

    let pk = PublicKey::from_slice(&pk.encode()).unwrap();

    Some(pk2id(&pk))
}

/// Converts a [`PeerId`] to a [`enr::NodeId`].
pub fn discv4_id_to_discv5_id(peer_id: PeerId) -> Result<NodeId, secp256k1::Error> {
    Ok(id2pk(peer_id)?.into())
}

/// Converts a [`PeerId`] to a [`libp2p_identity::PeerId`].
pub fn discv4_id_to_multiaddr_id(
    peer_id: PeerId,
) -> Result<libp2p_identity::PeerId, secp256k1::Error> {
    let pk = id2pk(peer_id)?.encode();
    let pk: libp2p_identity::PublicKey =
        libp2p_identity::secp256k1::PublicKey::try_from_bytes(&pk).unwrap().into();

    Ok(pk.to_peer_id())
}

/// Wrapper around [`discv5::Enr`] ([`Enr<CombinedKey>`]).
#[derive(Debug, Clone)]
pub struct EnrCombinedKeyWrapper(pub discv5::Enr);

impl TryFrom<Enr<SecretKey>> for EnrCombinedKeyWrapper {
    type Error = alloy_rlp::Error;

    fn try_from(value: Enr<SecretKey>) -> Result<Self, Self::Error> {
        // The k256 backend of `discv5::Enr` rejects some public key encodings that libsecp256k1
        // accepts.
        let encoded_enr = alloy_rlp::encode(&value);
        alloy_rlp::Decodable::decode(&mut &encoded_enr[..]).map(Self)
    }
}

impl From<EnrCombinedKeyWrapper> for Enr<SecretKey> {
    fn from(val: EnrCombinedKeyWrapper) -> Self {
        let encoded_enr = alloy_rlp::encode(&val.0);
        alloy_rlp::Decodable::decode(&mut &encoded_enr[..]).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::keccak256;
    use alloy_rlp::{Encodable, Header};
    use enr::{CombinedKey, EnrKey};
    use reth_chainspec::{EthereumHardfork, MAINNET};
    use reth_network_peers::NodeRecord;

    #[test]
    fn discv5_discv4_id_conversion() {
        let discv5_pk = CombinedKey::generate_secp256k1().public();
        let discv5_peer_id = NodeId::from(discv5_pk.clone());

        // convert to discv4 id
        let pk = secp256k1::PublicKey::from_slice(&discv5_pk.encode()).unwrap();
        let discv4_peer_id = pk2id(&pk);
        // convert back to discv5 id
        let discv5_peer_id_from_discv4_peer_id = discv4_id_to_discv5_id(discv4_peer_id).unwrap();

        assert_eq!(discv5_peer_id, discv5_peer_id_from_discv4_peer_id)
    }

    #[test]
    fn conversion_to_node_record_from_enr() {
        const IP: &str = "::";
        const TCP_PORT: u16 = 30303;
        const UDP_PORT: u16 = 9000;

        let key = CombinedKey::generate_secp256k1();

        let mut buf = Vec::new();
        let fork_id = MAINNET.hardfork_fork_id(EthereumHardfork::Frontier);
        fork_id.unwrap().encode(&mut buf);

        let enr = Enr::builder()
            .ip6(IP.parse().unwrap())
            .udp6(UDP_PORT)
            .tcp6(TCP_PORT)
            .build(&key)
            .unwrap();

        let enr = EnrCombinedKeyWrapper(enr).into();
        let node_record = NodeRecord::try_from(&enr).unwrap();

        assert_eq!(
            NodeRecord {
                address: IP.parse().unwrap(),
                tcp_port: TCP_PORT,
                udp_port: UDP_PORT,
                id: pk2id(&enr.public_key())
            },
            node_record
        )
    }

    #[test]
    fn enr_conversion_rejects_hybrid_encoded_key() {
        let sk = SecretKey::from_secret_bytes([1; 32]).unwrap();
        // Hybrid encoding: an uncompressed point with the y parity in the prefix byte.
        let mut pk = sk.public_key().serialize_uncompressed();
        pk[0] = 0x06 | (pk[64] & 1);

        let rlp_list = |payload: &[u8]| {
            let mut out = Vec::new();
            Header { list: true, payload_length: payload.len() }.encode(&mut out);
            out.extend_from_slice(payload);
            out
        };
        let mut content = Vec::new();
        1u64.encode(&mut content);
        for item in [&b"id"[..], b"v4", b"secp256k1", &pk] {
            item.encode(&mut content);
        }
        let msg = secp256k1::Message::from_digest(keccak256(rlp_list(&content)).0);
        let mut payload = Vec::new();
        secp256k1::ecdsa::sign(msg, &sk).serialize_compact().as_slice().encode(&mut payload);
        payload.extend_from_slice(&content);

        let enr = alloy_rlp::decode_exact::<Enr<SecretKey>>(rlp_list(&payload)).unwrap();
        assert_eq!(
            EnrCombinedKeyWrapper::try_from(enr).unwrap_err(),
            alloy_rlp::Error::Custom("Unknown signature")
        );
    }
}
