use crate::{Header, HeaderLocked, Receipt, Transaction, TransactionSigned};

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

/// Sealing Ethereum full block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockLocked {
    /// Locked block header.
    pub header: HeaderLocked,
    /// Transactions with signatures.
    pub body: Vec<TransactionSigned>,
    /// Block receipts.
    pub receipts: Vec<Receipt>,
}
