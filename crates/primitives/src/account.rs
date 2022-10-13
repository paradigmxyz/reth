use crate::{H256, U256};
use serde::{Deserialize, Serialize};

/// Account saved in database
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
    /// Hash of the bytecode.
    pub bytecode_hash: H256,
}
