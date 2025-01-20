//! Defines the types for blob transactions, legacy, and other EIP-2718 transactions included in a
//! response to `GetPooledTransactions`.

use crate::RecoveredTx;
use alloy_consensus::transaction::PooledTransaction;

/// A signed pooled transaction with recovered signer.
pub type PooledTransactionsElementEcRecovered<T = PooledTransaction> = RecoveredTx<T>;
