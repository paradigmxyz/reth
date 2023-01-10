use std::collections::HashMap;

use crate::{
    keccak256,
    proofs::{KeccakHasher, EMPTY_ROOT},
    Bytes, H256, KECCAK_EMPTY, U256,
};
use bytes::BytesMut;
use reth_codecs::{main_codec, Compact};
use reth_rlp::{length_of_length, Encodable, Header};
use serde::{Deserialize, Serialize};
use triehash::sec_trie_root;

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

impl GenesisAccount {
    /// Determines the RLP payload length, without the RLP header.
    fn payload_len(&self) -> usize {
        let mut len = 0;
        len += self.nonce.unwrap_or_default().length();
        len += self.balance.length();
        // rather than rlp-encoding the storage, we just return the length of a single hash
        // hashes are a fixed size, so it is safe to use the empty root for this
        len += EMPTY_ROOT.length();
        len += self.code.as_ref().map_or(KECCAK_EMPTY, keccak256).length();
        len
    }
}

impl Encodable for GenesisAccount {
    fn length(&self) -> usize {
        let len = self.payload_len();
        // RLP header length + payload length
        len + length_of_length(len)
    }
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        let header = Header { list: true, payload_length: self.payload_len() };
        header.encode(out);

        self.nonce.unwrap_or_default().encode(out);
        self.balance.encode(out);
        self.storage
            .as_ref()
            .map_or(EMPTY_ROOT, |storage| {
                let storage_values =
                    storage.iter().filter(|(_k, &v)| v != KECCAK_EMPTY).map(|(&k, v)| {
                        let mut value_rlp = BytesMut::new();
                        v.encode(&mut value_rlp);
                        (k, value_rlp.freeze())
                    });

                sec_trie_root::<KeccakHasher, _, _, _>(storage_values)
            })
            .encode(out);
        self.code.as_ref().map_or(KECCAK_EMPTY, keccak256).encode(out);
    }
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
