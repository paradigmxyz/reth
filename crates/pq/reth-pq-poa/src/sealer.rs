//! Block sealing: sign and verify block headers with ML-DSA-65.
//!
//! The seal covers the full RLP encoding of the header with `extra_data`
//! cleared (set to empty bytes). This binds the seal to ALL header fields
//! (parent hash, state root, transactions root, receipts root, etc.)
//! while excluding the seal itself (which lives in `extra_data`).

use alloy_consensus::Header;
use alloy_primitives::Bytes;
use alloy_rlp::Encodable;
use dilithium::{
    EncodedSignature, EncodedVerifyingKey, MlDsa65, Signature, SigningKey, VerifyingKey,
    dilithium65,
};
use dilithium::signature::SignatureEncoding;
use sha3::{
    Shake256,
    digest::{ExtendableOutput, Update, XofReader},
};
use thiserror::Error;

/// Errors during seal operations.
#[derive(Debug, Error)]
pub enum SealError {
    /// The seal has an invalid length (expected 3309 bytes for ML-DSA-65).
    #[error("invalid seal length: expected 3309 bytes, got {0}")]
    InvalidLength(usize),

    /// The ML-DSA-65 signature verification failed.
    #[error("signature verification failed")]
    InvalidSignature,

    /// The verifying key bytes could not be decoded.
    #[error("verifying key decode failed")]
    InvalidPublicKey,
}

/// Compute the seal hash of a block header.
///
/// The seal hash is `SHAKE-256(RLP(header_with_empty_extra_data))`.
/// This ensures the seal binds to every header field (parent hash, state root,
/// transactions root, block number, timestamp, gas limit, etc.) while
/// excluding the seal itself (which occupies `extra_data`).
pub fn header_seal_hash(header: &Header) -> [u8; 32] {
    let mut h = header.clone();
    h.extra_data = Bytes::new();
    let mut rlp_buf = Vec::with_capacity(h.length());
    h.encode(&mut rlp_buf);
    header_hash(&rlp_buf)
}

/// Compute a SHAKE-256 hash of arbitrary bytes (32-byte output).
pub fn header_hash(header_bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Shake256::default();
    hasher.update(header_bytes);
    let mut out = [0u8; 32];
    hasher.finalize_xof().read(&mut out);
    out
}

/// Seal a block header: sign its hash with the validator's ML-DSA-65 key.
/// Returns the 3309-byte signature.
pub fn seal_header(sk: &SigningKey<MlDsa65>, header_bytes: &[u8]) -> Vec<u8> {
    let hash = header_hash(header_bytes);
    let sig = dilithium65::sign(sk, &hash);
    sig.to_bytes().to_vec()
}

/// Verify a block seal against the expected validator's public key.
pub fn verify_seal(pk_bytes: &[u8], header_bytes: &[u8], seal: &[u8]) -> Result<(), SealError> {
    // Check seal length (ML-DSA-65 signature = 3309 bytes)
    if seal.len() != 3309 {
        return Err(SealError::InvalidLength(seal.len()));
    }

    // Decode the verifying key
    let encoded_pk = EncodedVerifyingKey::<MlDsa65>::try_from(pk_bytes)
        .map_err(|_| SealError::InvalidPublicKey)?;
    let pk = VerifyingKey::<MlDsa65>::decode(&encoded_pk);

    // Decode the signature
    let encoded_sig = EncodedSignature::<MlDsa65>::try_from(seal)
        .map_err(|_| SealError::InvalidSignature)?;
    let sig = Signature::<MlDsa65>::decode(&encoded_sig)
        .ok_or(SealError::InvalidSignature)?;

    // Compute header hash and verify
    let hash = header_hash(header_bytes);
    dilithium65::verify(&pk, &hash, &sig).map_err(|_| SealError::InvalidSignature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dilithium::signature::Keypair;

    #[test]
    fn seal_and_verify_roundtrip() {
        let sk = dilithium65::keygen();
        let vk = sk.verifying_key();
        let header = b"block header content for testing";

        let seal = seal_header(&sk, header);
        assert_eq!(seal.len(), 3309);

        let vk_bytes = vk.encode();
        verify_seal(vk_bytes.as_slice(), header, &seal).expect("valid seal must verify");
    }

    #[test]
    fn seal_and_verify_with_real_header() {
        let sk = dilithium65::keygen();
        let vk = sk.verifying_key();

        // Build a realistic header
        let mut header = Header::default();
        header.number = 42;
        header.gas_limit = 30_000_000;
        header.timestamp = 1_700_000_000;
        header.parent_hash = alloy_primitives::B256::repeat_byte(0xAB);

        // Compute seal hash (RLP of header with empty extra_data)
        let hash = header_seal_hash(&header);

        // Sign the hash
        let seal = seal_header(&sk, &hash);
        assert_eq!(seal.len(), 3309);

        // Verify using the same hash
        let vk_bytes = vk.encode();
        verify_seal(vk_bytes.as_slice(), &hash, &seal)
            .expect("valid seal over real header must verify");
    }

    #[test]
    fn header_seal_hash_excludes_extra_data() {
        let mut h1 = Header::default();
        h1.number = 10;
        h1.extra_data = Bytes::from(vec![0xDE, 0xAD]);

        let mut h2 = Header::default();
        h2.number = 10;
        h2.extra_data = Bytes::from(vec![0xBE, 0xEF, 0xFF]);

        // Different extra_data must produce the same seal hash
        assert_eq!(header_seal_hash(&h1), header_seal_hash(&h2));
    }

    #[test]
    fn header_seal_hash_changes_with_fields() {
        let mut h1 = Header::default();
        h1.number = 10;
        h1.parent_hash = alloy_primitives::B256::repeat_byte(0x01);

        let mut h2 = Header::default();
        h2.number = 10;
        h2.parent_hash = alloy_primitives::B256::repeat_byte(0x02);

        // Different parent_hash must produce different seal hashes
        assert_ne!(header_seal_hash(&h1), header_seal_hash(&h2));
    }

    #[test]
    fn tampered_header_fails() {
        let sk = dilithium65::keygen();
        let vk = sk.verifying_key();
        let header = b"original header";
        let tampered = b"tampered header";

        let seal = seal_header(&sk, header);
        let vk_bytes = vk.encode();

        let result = verify_seal(vk_bytes.as_slice(), tampered, &seal);
        assert!(result.is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let sk1 = dilithium65::keygen();
        let sk2 = dilithium65::keygen();
        let header = b"some header";

        let seal = seal_header(&sk1, header);
        let wrong_vk_bytes = sk2.verifying_key().encode();

        let result = verify_seal(wrong_vk_bytes.as_slice(), header, &seal);
        assert!(result.is_err());
    }

    #[test]
    fn tampered_real_header_fails_seal() {
        let sk = dilithium65::keygen();
        let vk = sk.verifying_key();

        let mut header = Header::default();
        header.number = 100;
        header.gas_limit = 30_000_000;
        header.parent_hash = alloy_primitives::B256::repeat_byte(0xAA);

        // Seal the original header
        let hash = header_seal_hash(&header);
        let seal = seal_header(&sk, &hash);

        // Tamper: change the state root
        header.state_root = alloy_primitives::B256::repeat_byte(0xFF);
        let tampered_hash = header_seal_hash(&header);

        // Verify against tampered hash must fail
        let vk_bytes = vk.encode();
        let result = verify_seal(vk_bytes.as_slice(), &tampered_hash, &seal);
        assert!(result.is_err(), "seal must fail after state_root tamper");
    }
}
