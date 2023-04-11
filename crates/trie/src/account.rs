use reth_primitives::{proofs::EMPTY_ROOT, Account, H256, KECCAK_EMPTY, U256};
use reth_rlp::{RlpDecodable, RlpEncodable};

/// An Ethereum account as represented in the trie.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
pub struct EthAccount {
    /// Account nonce.
    nonce: u64,
    /// Account balance.
    balance: U256,
    /// Account's storage root.
    storage_root: H256,
    /// Hash of the account's bytecode.
    code_hash: H256,
}

impl From<Account> for EthAccount {
    fn from(acc: Account) -> Self {
        EthAccount {
            nonce: acc.nonce,
            balance: acc.balance,
            storage_root: EMPTY_ROOT,
            code_hash: acc.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        }
    }
}

impl EthAccount {
    /// Set storage root on account.
    pub fn with_storage_root(mut self, storage_root: H256) -> Self {
        self.storage_root = storage_root;
        self
    }

    /// Get account's storage root.
    pub fn storage_root(&self) -> H256 {
        self.storage_root
    }
}
