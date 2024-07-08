#![allow(dead_code)]

use chacha20poly1305::{
    aead::{Aead, OsRng},
    ChaCha20Poly1305, KeyInit, Nonce,
};
use k256::{
    ecdsa::{SigningKey, VerifyingKey},
    elliptic_curve::{group::GroupEncoding, rand_core::RngCore, sec1::ToEncodedPoint},
    EncodedPoint, Secp256k1,
};
use reth::primitives::{keccak256, Address, B256};
use std::ops::Mul;

const NONCE_LEN: usize = 12;

type ProjectivePoint = k256::elliptic_curve::ProjectivePoint<Secp256k1>;

fn shared_secret(pk: &SigningKey, pb: &VerifyingKey) -> B256 {
    keccak256(
        ProjectivePoint::from(pb.as_affine())
            .mul(pk.as_nonzero_scalar().as_ref())
            .to_affine()
            .to_bytes(),
    )
}

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

/// Generates a stealth address from the given keys, alongside a view tag.
pub fn stealth_address(
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

/// Tries to decrypt a secure note by using the view private key and the ephemeral public one.
///
/// ```text
///       ┌────────────┬────────────────────────┐
/// data: │ nonce (12B)│ ciphertext (remaining) │
///       └────────────┴────────────────────────┘
/// ```
pub fn try_decrypt_node(view: &SigningKey, eph_pub: &VerifyingKey, data: &[u8]) -> Option<String> {
    if data.len() <= NONCE_LEN {
        return None
    }

    let nonce = Nonce::from_slice(&data[..NONCE_LEN]);
    let key = keccak256(shared_secret(view, eph_pub));
    let cipher = ChaCha20Poly1305::new(key.as_slice().into());

    let decrypted_plaintext = cipher.decrypt(&nonce, &data[NONCE_LEN..]).ok()?;
    String::from_utf8(decrypted_plaintext).ok()
}

/// Encrypts a secure note by using the ephemeral private key and the view public one.
///
/// Returns:
/// ```text
///  ┌────────────┬────────────────────────┐
///  │ nonce (12B)│ ciphertext (remaining) │
///  └────────────┴────────────────────────┘
/// ```
pub fn try_encrypt_note(
    eph: &SigningKey,
    view_pub: &VerifyingKey,
    plaintext: &str,
) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut buf);

    let nonce = Nonce::from_slice(&buf);
    let key = keccak256(shared_secret(eph, view_pub));
    let cipher = ChaCha20Poly1305::new(key.as_slice().into());

    buf.extend(cipher.encrypt(&nonce, plaintext.as_bytes()).ok()?);
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_roundtrip() {
        let view = SigningKey::from_slice(B256::random().as_slice()).unwrap();
        let eph = SigningKey::from_slice(B256::random().as_slice()).unwrap();
        let plaintext = "Stealthy".to_string();

        let cipher_text = try_encrypt_note(&eph, &view.verifying_key(), &plaintext).unwrap();

        assert_eq!(
            Some(plaintext),
            try_decrypt_node(&view, &eph.verifying_key(), &cipher_text)
        );
    }

    #[test]
    fn test_stealth_roundtrip() {
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
