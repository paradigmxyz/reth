use crate::{Header, Receipt, SealedHeader, Transaction, TransactionSigned, H256};
use std::ops::Deref;

/// Ethereum full block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Block {
    /// Block header.
    pub header: Header,
    /// Transactions in this block.
    pub body: Vec<Transaction>,
    /// Block receipts.
    pub receipts: Vec<Receipt>,
    /// Ommers/uncles header
    pub ommers: Vec<SealedHeader>,
}

impl Deref for Block {
    type Target = Header;
    fn deref(&self) -> &Self::Target {
        &self.header
    }
}

/// Sealed Ethereum full block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockLocked {
    /// Locked block header.
    pub header: SealedHeader,
    /// Transactions with signatures.
    pub body: Vec<TransactionSigned>,
    /// Block receipts.
    pub receipts: Vec<Receipt>,
    /// Omners/uncles header
    pub ommers: Vec<SealedHeader>,
}

impl BlockLocked {
    /// Header hash.
    pub fn hash(&self) -> H256 {
        self.header.hash()
    }
}

impl Deref for BlockLocked {
    type Target = Header;
    fn deref(&self) -> &Self::Target {
        self.header.as_ref()
    }
}
