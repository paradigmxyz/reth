//! Trait for checking type of transaction envelope.

use alloy_consensus::{transaction::EthereumTxEnvelope, TxType};
#[cfg(feature = "op")]
use op_alloy_consensus::transaction::OpTxEnvelope;
use op_alloy_consensus::OpTxType;

/// Trait for checking if a transaction envelope supports a given EIP-2718 type ID.
pub trait IsTyped2718 {
    /// Returns true if the given type ID corresponds to a supported typed transaction.
    fn is_type(type_id: u8) -> bool;
}

impl IsTyped2718 for TxType {
    fn is_type(type_id: u8) -> bool {
        matches!(type_id, 0x01 | 0x02 | 0x03 | 0x04 | 0x7E)
    }
}

impl<T: IsTyped2718> IsTyped2718 for EthereumTxEnvelope<T> {
    fn is_type(type_id: u8) -> bool {
        T::is_type(type_id)
    }
}

#[cfg(feature = "op")]
impl IsTyped2718 for OpTxType {
    fn is_type(type_id: u8) -> bool {
        matches!(type_id, 0x01 | 0x02 | 0x03 | 0x04 | 0x7E)
    }
}

#[cfg(feature = "op")]
impl IsTyped2718 for OpTxEnvelope {
    fn is_type(type_id: u8) -> bool {
        OpTxType::is_type(type_id)
    }
}
