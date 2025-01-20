//! Transaction types.

use crate::RecoveredTx;
pub use alloy_consensus::transaction::PooledTransaction;
use once_cell as _;
#[allow(deprecated)]
pub use pooled::PooledTransactionsElementEcRecovered;
pub use reth_primitives_traits::{
    sync::{LazyLock, OnceLock},
    transaction::{
        error::{
            InvalidTransactionError, TransactionConversionError, TryFromRecoveredTransactionError,
        },
        signed::SignedTransactionIntoRecoveredExt,
    },
    FillTxEnv, WithEncoded,
};
pub use signature::{recover_signer, recover_signer_unchecked};
pub use tx_type::TxType;

/// Handling transaction signature operations, including signature recovery,
/// applying chain IDs, and EIP-2 validation.
pub mod signature;
pub mod util;

mod pooled;
mod tx_type;

/// Signed transaction.
pub use reth_ethereum_primitives::{Transaction, TransactionSigned};

/// Type alias kept for backward compatibility.
#[deprecated(note = "Use `Recovered` instead")]
pub type TransactionSignedEcRecovered<T = TransactionSigned> = RecoveredTx<T>;
