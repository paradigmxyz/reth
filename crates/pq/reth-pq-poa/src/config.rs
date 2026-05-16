//! Global `PoA` configuration.
//!
//! Provides process-wide singletons for the validator set and signing key,
//! set once at startup from `main.rs` and read by the consensus builder
//! and payload builder without threading them through reth's generic pipeline.

use std::sync::OnceLock;

use dilithium::{KeyGen, MlDsa65, SigningKey};
use hybrid_array::Array;
use typenum::U32;

use crate::validator::ValidatorSet;

/// Global validator set — set once from `main.rs`.
static VALIDATOR_SET: OnceLock<ValidatorSet> = OnceLock::new();

/// Global signing key — set once from `main.rs` when this node is a validator.
static SIGNING_KEY: OnceLock<SigningKey<MlDsa65>> = OnceLock::new();

/// Store the validator set for use by `PqConsensusBuilder`.
///
/// # Panics
///
/// Panics if called more than once.
pub fn set_validator_set(vs: ValidatorSet) {
    VALIDATOR_SET
        .set(vs)
        .expect("validator set already initialized");
}

/// Retrieve the global validator set (returns `None` if not in `PoA` mode).
pub fn get_validator_set() -> Option<&'static ValidatorSet> {
    VALIDATOR_SET.get()
}

/// Store the signing key for use by the payload builder (block sealing).
///
/// The key is derived from a 32-byte seed (the raw ML-DSA-65 seed).
///
/// # Panics
///
/// Panics if called more than once.
pub fn set_signing_key(sk: SigningKey<MlDsa65>) {
    SIGNING_KEY
        .set(sk)
        .expect("signing key already initialized");
}

/// Derive an ML-DSA-65 signing key from a 32-byte seed.
///
/// This is the inverse of `SigningKey::to_seed()`.
pub fn signing_key_from_seed(seed_bytes: &[u8; 32]) -> SigningKey<MlDsa65> {
    let seed: Array<u8, U32> = (*seed_bytes).into();
    MlDsa65::from_seed(&seed)
}

/// Retrieve the global signing key (returns `None` if this node is not a validator).
pub fn get_signing_key() -> Option<&'static SigningKey<MlDsa65>> {
    SIGNING_KEY.get()
}
