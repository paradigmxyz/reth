//! # reth-pq-poa
//!
//! Proof of Authority consensus engine for `PostQuantumEVM`.
//! Validators sign blocks with ML-DSA-65 in round-robin rotation.
//!
//! ## Integration with reth
//!
//! - [`mining::PoaMiningStream`] integrates with reth's `MiningMode::Trigger`
//!   to drive block production only when this node is the authorized proposer.
//! - [`consensus::PqPoaConsensus`] wraps standard consensus with ML-DSA-65
//!   seal verification on incoming blocks.

pub mod config;
pub mod consensus;
pub mod engine;
pub mod mining;
pub mod sealer;
pub mod validator;

pub use config::{get_signing_key, get_validator_set, set_signing_key, set_validator_set, signing_key_from_seed};
pub use consensus::PqPoaConsensus;
pub use engine::{PoaConfig, PoaEngine};
pub use mining::PoaMiningStream;
pub use sealer::{header_seal_hash, seal_header, verify_seal};
pub use validator::{Validator, ValidatorSet};
