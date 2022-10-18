use reth_rlp::{RlpEncodable, RlpDecodable};

use crate::{Header, HeaderLocked, Receipt, Transaction, TransactionSigned, H256};
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
    pub header: HeaderLocked,
    /// Transactions with signatures.
    pub body: Vec<TransactionSigned>,
    /// Block receipts.
    pub receipts: Vec<Receipt>,
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

/// A block body, including transactions and uncle headers.
#[derive(Debug, Clone, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
pub struct BlockBody {
    /// This block's header
    pub header: Header,
    /// Transactions in this block.
    pub transactions: Vec<TransactionSigned>,
    /// Uncle block headers.
    pub ommers: Vec<Header>,
}
