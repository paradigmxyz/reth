#![allow(dead_code)]

use k256::{
    ecdsa::{SigningKey, VerifyingKey},
    elliptic_curve::group::GroupEncoding,
    EncodedPoint, Secp256k1,
};
use reth::primitives::{keccak256, B256};
use std::ops::Mul;

type ProjectivePoint = k256::elliptic_curve::ProjectivePoint<Secp256k1>;

/// Converts a raw public key to [`VerifyingKey`].
pub fn to_verifying_key(key: &[u8]) -> Option<VerifyingKey> {
    // Ensure the key is compressed or uncompressed
    if !(key.len() == 33 || key.len() == 65) {
        return None;
    }
    let encoded_point = EncodedPoint::from_bytes(key).ok()?;

    VerifyingKey::from_encoded_point(&encoded_point).ok()
}

/// Checks if given this view public key and ephemeral public key, the view tag matches.
pub fn is_ours(view: &SigningKey, eph_pub: &VerifyingKey, expected_view_tag: u8) -> bool {
    let shared_bytes = shared_secret(view, eph_pub);
    // first byte is the msb: big endian
    expected_view_tag == shared_bytes[0]
}

/// Generates the corresponding stealth address private key if the view tag matches.
pub fn stealth_key(
    spend: &SigningKey,
    view: &SigningKey,
    eph_pub: &VerifyingKey,
    expected_view_tag: u8,
) -> Option<SigningKey> {
    let shared_bytes = shared_secret(view, eph_pub);

    if expected_view_tag == shared_bytes[0] {
        let shared = SigningKey::from_slice(shared_bytes.as_slice()).ok()?;

        let combined_scalar =
            spend.as_nonzero_scalar().as_ref() + shared.as_nonzero_scalar().as_ref();
        return SigningKey::from_bytes(&combined_scalar.to_bytes()).ok()
    }

    None
}

fn shared_secret(pk: &SigningKey, pb: &VerifyingKey) -> B256 {
    keccak256(
        ProjectivePoint::from(pb.as_affine())
            .mul(pk.as_nonzero_scalar().as_ref())
            .to_affine()
            .to_bytes(),
    )
}

#[cfg(test)]
mod tests {

    use super::*;
    use k256::elliptic_curve::sec1::ToEncodedPoint;
    use reth::primitives::Address;

    fn stealth_address(
        spend_pub: &VerifyingKey,
        view_pub: &VerifyingKey,
        eph: &SigningKey,
    ) -> (Address, u8) {
        let shared_bytes = shared_secret(eph, view_pub);
        let shared = SigningKey::from_slice(shared_bytes.as_slice()).unwrap();
        let stealth_pub = ProjectivePoint::from(shared.verifying_key().as_affine()) +
            ProjectivePoint::from(spend_pub.as_affine());

        // first byte is the msb: big endian
        let tag = shared_bytes[0];
        (Address::from_raw_public_key(&stealth_pub.to_encoded_point(false).to_bytes()[1..]), tag)
    }

    #[test]
    fn test() {
        let spend = SigningKey::from_slice(B256::random().as_slice()).unwrap();
        let view = SigningKey::from_slice(B256::random().as_slice()).unwrap();
        let eph = SigningKey::from_slice(B256::random().as_slice()).unwrap();

        let (sth_addr, tag) = stealth_address(spend.verifying_key(), view.verifying_key(), &eph);

        let key = stealth_key(&spend, &view, eph.verifying_key(), tag).unwrap();

        assert_eq!(
            sth_addr,
            Address::from_raw_public_key(
                &key.verifying_key().to_encoded_point(false).to_bytes()[1..]
            )
        );
    }
}
