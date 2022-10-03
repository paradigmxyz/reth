use crate::{Header, Receipt, Transaction};

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
