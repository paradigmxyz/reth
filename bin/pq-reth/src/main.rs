//! # pq-reth
//!
//! Post-Quantum Ethereum execution node.
//!
//! This binary launches a [`PqNode`] — a reth-based Ethereum execution client
//! that replaces ECDSA/secp256k1 with ML-DSA-65 (CRYSTALS-Dilithium) for
//! transaction signing and verification.
//!
//! ## Usage
//!
//! ```bash
//! # Start in dev mode (auto-mining, no consensus layer needed)
//! pq-reth node --dev --dev.block-time 5s --http --http.addr 0.0.0.0
//!
//! # Start in PoA mode (reads validator config from env or file)
//! PQ_POA_CONFIG=/path/to/poa.json pq-reth node --dev --http
//! ```
//!
//! ## `PoA` Configuration
//!
//! Set the `PQ_POA_CONFIG` environment variable to a JSON file with:
//! ```json
//! {
//!   "slot_time_ms": 5000,
//!   "local_address": "0x...",
//!   "validators": [
//!     { "address": "0x...", "public_key": "0x..." },
//!     ...
//!   ]
//! }
//! ```
//!
//! When `PQ_POA_CONFIG` is set, the node uses round-robin `PoA` consensus
//! (only mining on this validator's turn). Without it, the node falls back
//! to standard dev-mode interval mining.

#![allow(missing_docs)]

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

use clap::Parser;
use reth_engine_local::MiningMode;
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_pq_node::PqNode;
use reth_pq_poa::{PoaMiningStream, Validator, ValidatorSet};
use std::time::Duration;
use tracing::{info, warn};

/// `PoA` configuration loaded from JSON.
#[derive(serde::Deserialize)]
struct PoaConfigFile {
    /// Slot time in milliseconds (default: 5000).
    #[serde(default = "default_slot_time_ms")]
    slot_time_ms: u64,
    /// This node's validator address (hex, 20 bytes).
    local_address: String,
    /// List of authorized validators.
    validators: Vec<ValidatorEntry>,
}

#[derive(serde::Deserialize)]
struct ValidatorEntry {
    /// Validator address (hex, 0x-prefixed or not).
    address: String,
    /// Validator public key (hex, ML-DSA-65 verifying key).
    public_key: String,
}

const fn default_slot_time_ms() -> u64 {
    5000
}

/// Parse a hex string (with or without 0x prefix) into bytes.
fn hex_decode(s: &str) -> Vec<u8> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(s).expect("invalid hex in PoA config")
}

/// Try to load `PoA` configuration from the `PQ_POA_CONFIG` environment variable.
fn load_poa_config() -> Option<(ValidatorSet, [u8; 20], Duration)> {
    let path = std::env::var("PQ_POA_CONFIG").ok()?;
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read PQ_POA_CONFIG at {path}: {e}"));
    let config: PoaConfigFile =
        serde_json::from_str(&content).expect("Failed to parse PQ_POA_CONFIG JSON");

    let validators: Vec<Validator> = config
        .validators
        .iter()
        .map(|v| {
            let addr_bytes = hex_decode(&v.address);
            let mut address = [0u8; 20];
            address.copy_from_slice(&addr_bytes);
            Validator {
                address,
                public_key: hex_decode(&v.public_key),
            }
        })
        .collect();

    let local_bytes = hex_decode(&config.local_address);
    let mut local_address = [0u8; 20];
    local_address.copy_from_slice(&local_bytes);

    let slot_time = Duration::from_millis(config.slot_time_ms);
    let vs = ValidatorSet::new(validators);

    info!(
        target: "pq-reth::poa",
        validators = vs.len(),
        slot_time_ms = config.slot_time_ms,
        local = %hex::encode(local_address),
        "PoA configuration loaded"
    );

    Some((vs, local_address, slot_time))
}

/// Try to load the validator signing key from `PQ_VALIDATOR_SK` env var.
///
/// The value must be a 64-char hex string (32-byte ML-DSA-65 seed).
fn load_signing_key() -> Option<()> {
    let sk_hex = std::env::var("PQ_VALIDATOR_SK").ok()?;
    let sk_bytes = hex_decode(&sk_hex);
    assert!(
        sk_bytes.len() == 32,
        "PQ_VALIDATOR_SK must be 32 bytes (64 hex chars), got {} bytes",
        sk_bytes.len()
    );
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&sk_bytes);
    let sk = reth_pq_poa::signing_key_from_seed(&seed);

    // Derive address from the signing key for logging
    use dilithium::signature::Keypair;
    use sha3::{Shake256, digest::{ExtendableOutput, Update, XofReader}};
    let pk_bytes = sk.verifying_key().encode();
    let mut hasher = Shake256::default();
    hasher.update(pk_bytes.as_slice());
    let mut hash = [0u8; 32];
    hasher.finalize_xof().read(&mut hash);
    let addr_hex = hex::encode(&hash[12..32]);

    reth_pq_poa::set_signing_key(sk);
    info!(
        target: "pq-reth::poa",
        address = %addr_hex,
        "Validator signing key loaded (ML-DSA-65)"
    );

    Some(())
}

fn main() {
    reth_cli_util::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }

    if let Err(err) =
        reth_ethereum_cli::Cli::<EthereumChainSpecParser>::parse().run(async move |builder, _| {
            info!(target: "pq-reth::cli", "Launching Post-Quantum node (ML-DSA-65)");

            // Load signing key first (independent of PoA config — useful for
            // single-node dev mode too)
            load_signing_key();

            let handle = if let Some((validator_set, local_address, slot_time)) = load_poa_config()
            {
                // PoA mode: mine only on our turn using round-robin rotation
                info!(target: "pq-reth::cli", "Running in PoA consensus mode");

                // Set global validator set for PqConsensusBuilder
                reth_pq_poa::set_validator_set(validator_set.clone());

                let poa_stream = PoaMiningStream::new(
                    validator_set,
                    local_address,
                    slot_time,
                    1, // start_block: genesis is block 0, first to produce is 1
                );

                builder
                    .node(PqNode::default())
                    .launch_with_debug_capabilities()
                    .with_mining_mode(MiningMode::trigger(poa_stream))
                    .await?
            } else {
                // Standard dev mode: mine at fixed interval (--dev.block-time)
                warn!(
                    target: "pq-reth::cli",
                    "No PQ_POA_CONFIG set — using standard dev mining mode"
                );

                builder
                    .node(PqNode::default())
                    .launch_with_debug_capabilities()
                    .await?
            };

            handle.wait_for_node_exit().await
        })
    {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
