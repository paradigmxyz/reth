//! Interface between node identification on protocol version 5 and 4. Specifically, between types
//! [`discv5::enr::NodeId`] and [`PeerId`].

use discv5::enr::{CombinedPublicKey, Enr, EnrPublicKey, NodeId};
use reth_discv4::secp256k1::{constants::UNCOMPRESSED_PUBLIC_KEY_SIZE, PublicKey, SecretKey};
use reth_primitives::PeerId;

const SECP256K1_SERIALIZED_UNCOMPRESSED_FLAG: u8 = 4;

/// Extracts a [`CombinedPublicKey::Secp256k1`] from a [`discv5::Enr`] and converts it to a
/// [`PeerId`] that can be used in [`Discv4`](reth_discv4::Discv4).
pub fn uncompressed_id_from_enr_pk(enr: &discv5::Enr) -> PeerId {
    let pk = enr.public_key();
    debug_assert!(
        matches!(pk, CombinedPublicKey::Secp256k1(_)),
        "discv5 using different key type than discv4"
    );

    pk_to_uncompressed_id(&pk)
}

/// Converts a [`CombinedPublicKey`] to a [`PeerId`] that can be used in
/// [`Discv4`](reth_discv4::Discv4).
pub fn pk_to_uncompressed_id(pk: &CombinedPublicKey) -> PeerId {
    let pk_bytes = pk.encode();
    let pk = discv5::enr::secp256k1::PublicKey::from_slice(&pk_bytes).unwrap();

    PeerId::from_slice(&pk.serialize_uncompressed()[1..])
}

/// Converts a [`PeerId`] to a [`discv5::enr::NodeId`].
pub fn uncompressed_to_compressed_id(peer_id: PeerId) -> NodeId {
    uncompressed_id_to_pk(peer_id).into()
}

/// Converts a [`PeerId`] to a [`PublicKey`].
pub fn uncompressed_id_to_pk(peer_id: PeerId) -> PublicKey {
    let mut buf = [0u8; UNCOMPRESSED_PUBLIC_KEY_SIZE];
    buf[0] = SECP256K1_SERIALIZED_UNCOMPRESSED_FLAG;
    buf[1..].copy_from_slice(peer_id.as_ref());

    PublicKey::from_slice(&buf).unwrap()
}

/// Converts a [`PeerId`] to a [`libp2p_identity::PeerId `].
pub fn uncompressed_to_multiaddr_id(peer_id: PeerId) -> libp2p_identity::PeerId {
    let pk = uncompressed_id_to_pk(peer_id).encode();
    let pk: libp2p_identity::PublicKey =
        libp2p_identity::secp256k1::PublicKey::try_from_bytes(&pk).unwrap().into();

    pk.to_peer_id()
}

/// Wrapper around [`discv5::Enr`] ([`Enr<CombinedKey>`]).
#[derive(Debug, Clone)]
pub struct EnrCombinedKeyWrapper(pub discv5::Enr);

impl From<Enr<SecretKey>> for EnrCombinedKeyWrapper {
    fn from(value: Enr<SecretKey>) -> Self {
        let encoded_enr = rlp::encode(&value);
        let enr = rlp::decode::<discv5::Enr>(&encoded_enr).unwrap();

        Self(enr)
    }
}

impl From<EnrCombinedKeyWrapper> for Enr<SecretKey> {
    fn from(val: EnrCombinedKeyWrapper) -> Self {
        let EnrCombinedKeyWrapper(enr) = val;
        let encoded_enr = rlp::encode(&enr);

        rlp::decode::<Enr<SecretKey>>(&encoded_enr).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use discv5::enr::{CombinedKey, EnrKey};

    use super::*;

    #[test]
    fn discv5_discv4_id_conversion() {
        let discv5_pk = CombinedKey::generate_secp256k1().public();
        let discv5_peer_id = NodeId::from(discv5_pk.clone());

        // convert to discv4 id
        let discv4_peer_id = pk_to_uncompressed_id(&discv5_pk);
        // convert back to discv5 id
        let discv5_peer_id_from_discv4_peer_id = uncompressed_to_compressed_id(discv4_peer_id);

        assert_eq!(discv5_peer_id, discv5_peer_id_from_discv4_peer_id)
    }
}
