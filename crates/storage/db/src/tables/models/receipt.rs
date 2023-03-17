//! Receipt stored inside database.

use reth_codecs::{main_codec, Compact};
use reth_primitives::{bloom::logs_bloom, Log, Receipt, TxType};

/// Stored receipt data.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[main_codec]
pub struct StoredReceipt {
    /// Receipt type.
    pub tx_type: TxType,
    /// If transaction is executed successfully.
    ///
    /// This is the `statusCode`
    pub success: bool,
    /// Gas used
    pub cumulative_gas_used: u64,
    /// Log send from contracts.
    pub logs: Vec<Log>,
}

impl From<Receipt> for StoredReceipt {
    fn from(r: Receipt) -> Self {
        Self {
            tx_type: r.tx_type,
            success: r.success,
            cumulative_gas_used: r.cumulative_gas_used,
            logs: r.logs,
        }
    }
}

impl From<StoredReceipt> for Receipt {
    fn from(sr: StoredReceipt) -> Self {
        let bloom = logs_bloom(sr.logs.iter());
        Self {
            tx_type: sr.tx_type,
            success: sr.success,
            cumulative_gas_used: sr.cumulative_gas_used,
            logs: sr.logs,
            bloom,
        }
    }
}
