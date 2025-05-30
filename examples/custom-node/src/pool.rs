use crate::primitives::CustomTransactionEnvelope;
use op_alloy_consensus::OpPooledTransaction;
use reth_ethereum::primitives::Extended;

pub type CustomPooledTransaction = Extended<OpPooledTransaction, CustomTransactionEnvelope>;
