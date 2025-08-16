use crate::primitives::{CustomTransaction, CustomTransactionEnvelope};
use alloy_consensus::{
    crypto::RecoveryError, error::ValueError, transaction::SignerRecoverable, TransactionEnvelope,
};
use alloy_primitives::{Address, Sealed, B256};
use op_alloy_consensus::{OpPooledTransaction, OpTransaction, TxDeposit};
use reth_ethereum::primitives::{
    serde_bincode_compat::RlpBincode, InMemorySize, SignedTransaction,
};

#[derive(Clone, Debug, TransactionEnvelope)]
#[envelope(tx_type_name = CustomPooledTxType)]
pub enum CustomPooledTransaction {
    /// A regular Optimism transaction as defined by [`OpPooledTransaction`].
    #[envelope(flatten)]
    Op(OpPooledTransaction),
    /// A [`CustomTransactionEnvelope`] tagged with type 0x7E.
    #[envelope(ty = 42)]
    Payment(CustomTransactionEnvelope),
}

impl From<CustomPooledTransaction> for CustomTransaction {
    fn from(tx: CustomPooledTransaction) -> Self {
        match tx {
            CustomPooledTransaction::Op(tx) => Self::Op(tx.into()),
            CustomPooledTransaction::Payment(tx) => Self::Payment(tx),
        }
    }
}

impl TryFrom<CustomTransaction> for CustomPooledTransaction {
    type Error = ValueError<CustomTransaction>;

    fn try_from(tx: CustomTransaction) -> Result<Self, Self::Error> {
        match tx {
            CustomTransaction::Op(op) => Ok(Self::Op(
                OpPooledTransaction::try_from(op).map_err(|op| op.map(CustomTransaction::Op))?,
            )),
            CustomTransaction::Payment(payment) => Ok(Self::Payment(payment)),
        }
    }
}

impl RlpBincode for CustomPooledTransaction {}

impl OpTransaction for CustomPooledTransaction {
    fn is_deposit(&self) -> bool {
        match self {
            CustomPooledTransaction::Op(_) => false,
            CustomPooledTransaction::Payment(payment) => payment.is_deposit(),
        }
    }

    fn as_deposit(&self) -> Option<&Sealed<TxDeposit>> {
        match self {
            CustomPooledTransaction::Op(_) => None,
            CustomPooledTransaction::Payment(payment) => payment.as_deposit(),
        }
    }
}

impl SignerRecoverable for CustomPooledTransaction {
    fn recover_signer(&self) -> Result<Address, RecoveryError> {
        match self {
            CustomPooledTransaction::Op(tx) => SignerRecoverable::recover_signer(tx),
            CustomPooledTransaction::Payment(tx) => SignerRecoverable::recover_signer(tx),
        }
    }

    fn recover_signer_unchecked(&self) -> Result<Address, RecoveryError> {
        match self {
            CustomPooledTransaction::Op(tx) => SignerRecoverable::recover_signer_unchecked(tx),
            CustomPooledTransaction::Payment(tx) => SignerRecoverable::recover_signer_unchecked(tx),
        }
    }
}

impl SignedTransaction for CustomPooledTransaction {
    fn tx_hash(&self) -> &B256 {
        match self {
            CustomPooledTransaction::Op(tx) => SignedTransaction::tx_hash(tx),
            CustomPooledTransaction::Payment(tx) => SignedTransaction::tx_hash(tx),
        }
    }
}

impl InMemorySize for CustomPooledTransaction {
    fn size(&self) -> usize {
        match self {
            CustomPooledTransaction::Op(tx) => InMemorySize::size(tx),
            CustomPooledTransaction::Payment(tx) => InMemorySize::size(tx),
        }
    }
}
