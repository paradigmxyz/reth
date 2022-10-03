use crate::Header;
use crate::Receipt;
use crate::Transaction;

/// Ethereum full block

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Block {
    /// Block Header.
    pub header: Header,
    /// Block body with all transactions.
    pub body: Vec<Transaction>,
    /// Blcok receipts.
    pub receipts: Vec<Receipt>,
}
