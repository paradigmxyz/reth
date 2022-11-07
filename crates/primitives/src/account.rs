use crate::{H256, U256};
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
    #[maybe_zero]
    /// Hash of the bytecode.
    pub bytecode_hash: H256,
}

#[cfg(test)]
mod tests {
    use crate::Account;
    use reth_codecs::Compact;

    #[test]
    fn test_account() {
        let mut acc = Account::default();
        let (len, _) = acc.to_compact();
        assert_eq!(len, 2);

        acc.balance = 2.into();
        let (len, _) = acc.to_compact();
        assert_eq!(len, 3);

        acc.nonce = 2;
        let (len, _) = acc.to_compact();
        assert_eq!(len, 4);
    }
}
