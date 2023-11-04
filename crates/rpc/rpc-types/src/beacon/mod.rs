#![allow(missing_docs)]
//! Types for the Ethereum 2.0 RPC protocol (beacon chain)

use alloy_primitives::FixedBytes;
use constants::{BLS_PUBLIC_KEY_BYTES_LEN, BLS_SIGNATURE_BYTES_LEN};

pub mod constants;
pub mod macros;
pub mod payload;

/// BLS signature type
pub type BlsSignature = FixedBytes<BLS_SIGNATURE_BYTES_LEN>;

/// BLS public key type
pub type BlsPublicKey = FixedBytes<BLS_PUBLIC_KEY_BYTES_LEN>;
