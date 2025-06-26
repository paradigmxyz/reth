use crate::primitives::{CustomTransaction, CustomTransactionEnvelope};
use alloy_consensus::error::ValueError;
use op_alloy_consensus::OpPooledTransaction;
use reth_ethereum::primitives::Extended;

pub type CustomPooledTransaction = Extended<OpPooledTransaction, CustomTransactionEnvelope>;

impl From<CustomPooledTransaction> for CustomTransaction {
    fn from(tx: CustomPooledTransaction) -> Self {
        match tx {
            CustomPooledTransaction::BuiltIn(tx) => Self::Op(tx.into()),
            CustomPooledTransaction::Other(tx) => Self::Payment(tx),
        }
    }
}

impl TryFrom<CustomTransaction> for CustomPooledTransaction {
    type Error = ValueError<CustomTransaction>;

    fn try_from(tx: CustomTransaction) -> Result<Self, Self::Error> {
        match tx {
            CustomTransaction::Op(op) => Ok(Self::BuiltIn(
                OpPooledTransaction::try_from(op).map_err(|op| op.map(CustomTransaction::Op))?,
            )),
            CustomTransaction::Payment(payment) => Ok(Self::Other(payment)),
        }
    }
}
