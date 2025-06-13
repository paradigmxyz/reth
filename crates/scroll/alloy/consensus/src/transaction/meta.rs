use alloy_consensus::transaction::TransactionInfo;
use alloy_primitives::U256;

/// Additional receipt metadata required for Scroll transactions.
///
/// These fields are used to provide additional context for in RPC responses.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct ScrollAdditionalInfo {
    /// Only present in RPC responses.
    pub l1_fee: U256,
}

/// Additional fields in the context of a block that contains this transaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollTransactionInfo {
    /// Additional transaction information.
    pub inner: TransactionInfo,
    /// Additional metadata for Scroll.
    pub additional_info: ScrollAdditionalInfo,
}

impl ScrollTransactionInfo {
    /// Creates a new [`ScrollTransactionInfo`] with the given [`TransactionInfo`] and
    /// [`ScrollAdditionalInfo`].
    pub const fn new(inner: TransactionInfo, additional_info: ScrollAdditionalInfo) -> Self {
        Self { inner, additional_info }
    }
}
