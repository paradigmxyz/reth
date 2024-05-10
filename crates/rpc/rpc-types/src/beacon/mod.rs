//! Types for the Ethereum 2.0 RPC protocol (beacon chain).

#![allow(missing_docs)]

use alloy_primitives::FixedBytes;
use constants::{BLS_PUBLIC_KEY_BYTES_LEN, BLS_SIGNATURE_BYTES_LEN};

pub mod constants;
/// Beacon API events support.
pub mod events;
pub mod payload;
pub mod withdrawals;

/// BLS signature type
pub type BlsSignature = FixedBytes<BLS_SIGNATURE_BYTES_LEN>;

/// BLS public key type
pub type BlsPublicKey = FixedBytes<BLS_PUBLIC_KEY_BYTES_LEN>;
