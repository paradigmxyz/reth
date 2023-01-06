use std::collections::HashMap;

use crate::{Bytes, H256, U256};
use reth_codecs::{main_codec, Compact};
use serde::{Deserialize, Serialize};

/// An Ethereum account.
#[main_codec]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Account {
    /// Account nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
    /// Hash of the account's bytecode.
    pub bytecode_hash: Option<H256>,
}

/// An account in the state of the genesis block.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisAccount {
    /// The nonce of the account at genesis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<u64>,
    /// The balance of the account at genesis.
    pub balance: U256,
    /// The account's bytecode at genesis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<Bytes>,
    /// The account's storage at genesis.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub storage: Option<HashMap<H256, H256>>,
}

impl Account {
    /// Whether the account has bytecode.
    pub fn has_bytecode(&self) -> bool {
        self.bytecode_hash.is_some()
    }
}

#[cfg(test)]
mod tests {
    use crate::{Account, U256};
    use reth_codecs::Compact;

    #[test]
    fn test_account() {
        let mut buf = vec![];
        let mut acc = Account::default();
        let len = acc.to_compact(&mut buf);
        assert_eq!(len, 2);

        acc.balance = U256::from(2);
        let len = acc.to_compact(&mut buf);
        assert_eq!(len, 3);

        acc.nonce = 2;
        let len = acc.to_compact(&mut buf);
        assert_eq!(len, 4);
    }
}
