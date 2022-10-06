use crate::{Log, TxType, H256};

/// Receipt containing result of transaction execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    /// Receipt type.
    pub tx_type: TxType,
    /// If transaction is executed successfully.
    pub success: bool,
    /// Gas used
    pub cumulative_gas_used: u64,
    /// Bloom filter.
    pub bloom: H256,
    /// Log send from contracts.
    pub logs: Vec<Log>,
}
