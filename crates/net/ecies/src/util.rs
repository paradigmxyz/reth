//! Utility functions for hashing and encoding.

use crate::{ECIESError, ECIESErrorImpl};
use alloy_primitives::B256;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// Hashes the input data with SHA256 - <https://en.wikipedia.org/wiki/SHA-2>
pub(crate) fn sha256(data: &[u8]) -> B256 {
    B256::from(Sha256::digest(data).as_ref())
}

/// Produces a `HMAC_SHA256` digest of the `input_data` and `auth_data` with the given `key`.
/// This is done by accumulating each slice in `input_data` into the HMAC state, then accumulating
/// the `auth_data` and returning the resulting digest.
pub(crate) fn hmac_sha256(key: &[u8], input: &[&[u8]], auth_data: &[u8]) -> B256 {
    let mut hmac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    for input in input {
        hmac.update(input);
    }
    hmac.update(auth_data);
    B256::from_slice(&hmac.finalize().into_bytes())
}

/// Verifies an HMAC-SHA256 tag using constant-time comparison.
///
/// This function uses HMAC's built-in `verify` method which performs constant-time
/// comparison to prevent timing attacks.
pub(crate) fn verify_hmac_sha256(
    key: &[u8],
    input: &[&[u8]],
    auth_data: &[u8],
    expected_tag: &[u8],
) -> Result<(), ECIESError> {
    let mut hmac =
        Hmac::<Sha256>::new_from_slice(key).map_err(ECIESErrorImpl::InvalidHmacKeyLength)?;
    for input in input {
        hmac.update(input);
    }
    hmac.update(auth_data);
    hmac.verify_slice(expected_tag)
        .map_err(|_| ECIESError::from(ECIESErrorImpl::MacVerificationFailed))?;
    Ok(())
}
