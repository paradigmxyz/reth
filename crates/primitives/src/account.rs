use crate::{H256, KECCAK_EMPTY, U256};
use reth_codecs::{main_codec, Compact};

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

impl Account {
    /// Whether the account has bytecode.
    pub fn has_bytecode(&self) -> bool {
        self.bytecode_hash.is_some()
    }

    /// After SpuriousDragon empty account is defined as account with nonce == 0 && balance == 0 &&
    /// bytecode = None.
    pub fn is_empty(&self) -> bool {
        let is_bytecode_empty = match self.bytecode_hash {
            None => true,
            Some(hash) => hash == KECCAK_EMPTY,
        };

        self.nonce == 0 && self.balance == U256::ZERO && is_bytecode_empty
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
