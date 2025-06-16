//! Transaction types for Scroll.

mod tx_type;
pub use tx_type::{ScrollTxType, L1_MESSAGE_TX_TYPE_ID};

mod envelope;
pub use envelope::ScrollTxEnvelope;

mod l1_message;
pub use l1_message::{ScrollL1MessageTransactionFields, TxL1Message, L1_MESSAGE_TRANSACTION_TYPE};

mod meta;
pub use meta::{ScrollAdditionalInfo, ScrollTransactionInfo};

mod typed;
pub use typed::ScrollTypedTransaction;

mod pooled;
pub use pooled::ScrollPooledTransaction;

#[cfg(feature = "serde")]
pub use l1_message::serde_l1_message_tx_rpc;

/// Bincode-compatible serde implementations for transaction types.
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub(super) mod serde_bincode_compat {
    pub use super::envelope::serde_bincode_compat::*;
}
