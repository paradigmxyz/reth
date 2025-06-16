//! Scroll primitives transaction types.

pub mod tx_type;

/// Signed transaction.
pub type ScrollTransactionSigned = scroll_alloy_consensus::ScrollTxEnvelope;
