//! Interface between node identification on protocol version 5 and 4. Specifically, between types
//! [`discv5::enr::NodeId`] and [`PeerId`].

use discv5::enr::{CombinedPublicKey, Enr, EnrKey, EnrPublicKey, NodeId};
use reth_primitives::PeerId;
use secp256k1::{constants::UNCOMPRESSED_PUBLIC_KEY_SIZE, PublicKey, SecretKey};

const SECP256K1_SERIALIZED_UNCOMPRESSED_FLAG: u8 = 4;

/// Extracts a [`CombinedPublicKey::Secp256k1`] from a [`discv5::Enr`] and converts it to a
/// [`PeerId`] that can be used in [`Discv4`](reth_discv4::Discv4).
pub fn uncompressed_id_from_enr_pk(
    enr: &discv5::Enr,
) -> Result<PeerId, discv5::enr::secp256k1::Error> {
    let pk = enr.public_key();
    debug_assert!(
        matches!(pk, CombinedPublicKey::Secp256k1(_)),
        "discv5 using different key type than discv4"
    );

    pk_to_uncompressed_id(&pk)
}

/// Converts a [`CombinedPublicKey`] to a [`PeerId`] that can be used in
/// [`Discv4`](reth_discv4::Discv4).
pub fn pk_to_uncompressed_id(
    pk: &CombinedPublicKey,
) -> Result<PeerId, discv5::enr::secp256k1::Error> {
    let pk_bytes = pk.encode();

    let pk = discv5::enr::secp256k1::PublicKey::from_slice(&pk_bytes)?;
    let peer_id = PeerId::from_slice(&pk.serialize_uncompressed()[1..]);

    Ok(peer_id)
}

/// Converts a [`PeerId`] to a [`discv5::enr::NodeId`].
pub fn uncompressed_to_compressed_id(peer_id: PeerId) -> Result<NodeId, secp256k1::Error> {
    let mut buf = [0u8; UNCOMPRESSED_PUBLIC_KEY_SIZE];
    buf[0] = SECP256K1_SERIALIZED_UNCOMPRESSED_FLAG;
    buf[1..].copy_from_slice(peer_id.as_ref());
    let pk = PublicKey::from_slice(&buf)?;

    Ok(pk.into())
}

/// Checks if the fork id is set for an [`Enr`] received over [`discv5::Discv5`].
pub fn is_fork_id_set_for_discv5_peer<K: EnrKey>(enr: &Enr<K>) -> bool {
    enr.get("eth").is_some()
}

/// Wrapper around [`discv5::Enr`] ([`Enr<CombinedKey>`]).
#[derive(Debug, Clone)]
pub struct EnrCombinedKeyWrapper(pub discv5::Enr);

impl TryFrom<Enr<SecretKey>> for EnrCombinedKeyWrapper {
    type Error = rlp::DecoderError;
    fn try_from(value: Enr<SecretKey>) -> Result<Self, Self::Error> {
        let encoded_enr = rlp::encode(&value);
        let enr = rlp::decode::<discv5::Enr>(&encoded_enr)?;

        Ok(EnrCombinedKeyWrapper(enr))
    }
}

impl TryInto<Enr<SecretKey>> for EnrCombinedKeyWrapper {
    type Error = rlp::DecoderError;
    fn try_into(self) -> Result<Enr<SecretKey>, Self::Error> {
        let Self(enr) = self;
        let encoded_enr = rlp::encode(&enr);
        let enr = rlp::decode::<Enr<SecretKey>>(&encoded_enr)?;

        Ok(enr)
    }
}

#[cfg(test)]
mod tests {
    use discv5::enr::CombinedKey;

    use super::*;

    #[test]
    fn discv5_discv4_id_conversion() {
        let discv5_pk = CombinedKey::generate_secp256k1().public();
        let discv5_peer_id = NodeId::from(discv5_pk.clone());

        // convert to discv4 id
        let discv4_peer_id = pk_to_uncompressed_id(&discv5_pk).unwrap();
        // convert back to discv5 id
        let discv5_peer_id_from_discv4_peer_id =
            uncompressed_to_compressed_id(discv4_peer_id).unwrap();

        assert_eq!(discv5_peer_id, discv5_peer_id_from_discv4_peer_id)
    }
}
