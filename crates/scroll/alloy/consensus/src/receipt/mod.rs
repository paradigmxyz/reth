mod envelope;
pub use envelope::ScrollReceiptEnvelope;

#[allow(clippy::module_inception)]
mod receipt;
pub use receipt::{ScrollReceiptWithBloom, ScrollTransactionReceipt};
