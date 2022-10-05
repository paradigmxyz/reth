use crate::{Header, Receipt, Transaction};

/// Ethereum full block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Block {
    /// Block header.
    pub header: Header,
    /// Transactions in this block.
    pub body: Vec<Transaction>,
    /// Block receipts.
    pub receipts: Vec<Receipt>,
}
