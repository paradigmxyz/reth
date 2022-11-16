use crate::{H256, U256};
use reth_codecs::main_codec;

/// Account saved in database
#[main_codec]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Account {
    /// Nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
    /// Hash of the bytecode.
    pub bytecode_hash: Option<H256>,
}

impl Account {
    /// Does account has a bytecode.
    pub fn has_bytecode(&self) -> bool {
        self.bytecode_hash.is_some()
    }
}
