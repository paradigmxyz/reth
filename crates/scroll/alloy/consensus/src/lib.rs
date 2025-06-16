#![doc = include_str!("../README.md")]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/alloy-rs/core/main/assets/alloy.jpg",
    html_favicon_url = "https://raw.githubusercontent.com/alloy-rs/core/main/assets/favicon.ico"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc as std;

mod transaction;
pub use transaction::{
    ScrollL1MessageTransactionFields, ScrollPooledTransaction, ScrollTxEnvelope, ScrollTxType,
    ScrollTypedTransaction, TxL1Message, L1_MESSAGE_TRANSACTION_TYPE, L1_MESSAGE_TX_TYPE_ID,
};

mod receipt;
pub use receipt::{ScrollReceiptEnvelope, ScrollReceiptWithBloom, ScrollTransactionReceipt};

#[cfg(feature = "serde")]
pub use transaction::serde_l1_message_tx_rpc;
