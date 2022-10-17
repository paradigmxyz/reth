use crate::{H256, U256};
use reth_codecs::main_codec;

/// Account saved in database
#[main_codec]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Account {
    /// Nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
    /// Hash of the bytecode.
    pub bytecode_hash: H256,
}
