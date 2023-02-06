use std::collections::HashMap;

use crate::{
    keccak256,
    proofs::{KeccakHasher, EMPTY_ROOT},
    utils::serde_helpers::deserialize_stringified_u64,
    Address, Bytes, H256, KECCAK_EMPTY, U256,
};
use bytes::BytesMut;
use ethers_core::utils::GenesisAccount as EthersGenesisAccount;
use reth_rlp::{length_of_length, Encodable, Header as RlpHeader};
use serde::{Deserialize, Serialize};
use triehash::sec_trie_root;

/// The genesis block specification.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Genesis {
    /// The genesis header nonce.
    #[serde(deserialize_with = "deserialize_stringified_u64")]
    pub nonce: u64,
    /// The genesis header timestamp.
    #[serde(deserialize_with = "deserialize_stringified_u64")]
    pub timestamp: u64,
    /// The genesis header extra data.
    pub extra_data: Bytes,
    /// The genesis header gas limit.
    #[serde(deserialize_with = "deserialize_stringified_u64")]
    pub gas_limit: u64,
    /// The genesis header difficulty.
    pub difficulty: U256,
    /// The genesis header mix hash.
    pub mix_hash: H256,
    /// The genesis header coinbase address.
    pub coinbase: Address,
    /// The initial state of accounts in the genesis block.
    pub alloc: HashMap<Address, GenesisAccount>,
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
        let header = RlpHeader { list: true, payload_length: self.payload_len() };
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

impl From<EthersGenesisAccount> for GenesisAccount {
    fn from(genesis_account: EthersGenesisAccount) -> Self {
        Self {
            balance: genesis_account.balance.into(),
            nonce: genesis_account.nonce,
            code: genesis_account.code.as_ref().map(|code| code.0.clone().into()),
            storage: genesis_account.storage.as_ref().map(|storage| {
                storage.clone().into_iter().map(|(k, v)| (k.0.into(), v.0.into())).collect()
            }),
        }
    }
}
