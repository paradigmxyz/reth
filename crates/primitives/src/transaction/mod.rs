mod signature;
mod tx_type;

use signature::Signature;
pub use tx_type::TxType;

/// Raw Transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transaction {
    /// Legacy transaciton.
    Legacy {
        /// Nonce.
        nonce: u64,
    },
    /// Transaction with AccessList.
    Eip2930 {
        /// nonce.
        nonce: u64,
    },
    /// Transaction with priority fee.
    Eip1559 {
        /// Nonce.
        nonce: u64,
    },
}

/// Signed transaction.
#[derive(Debug, Clone)]
pub struct TransactionSigned {
    transaction: Transaction,
    signature: Signature,
}

impl AsRef<Transaction> for TransactionSigned {
    fn as_ref(&self) -> &Transaction {
        &self.transaction
    }
}

impl TransactionSigned {
    /// Transaction signature.
    pub fn signature(&self) -> &Signature {
        &self.signature
    }
}
