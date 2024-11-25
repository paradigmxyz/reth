//! Abstraction of an executable transaction.

use alloy_primitives::Address;
use revm_primitives::TxEnv;

/// Loads transaction into execution environment.
pub trait FillTxEnv {
    /// Fills [`TxEnv`] with an [`Address`] and transaction.
    fn fill_tx_env(&self, tx_env: &mut TxEnv, sender: Address);
}
