use crate::{Address, H256, U256};
use bytes::Buf;
use modular_bitfield::prelude::*;
use reth_codecs::{main_codec, Compact};

/// Account saved in database
#[main_codec]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Compact)]
pub struct Account {
    /// Nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
    /// Hash of the bytecode.
    pub bytecode_hash: H256,
}
