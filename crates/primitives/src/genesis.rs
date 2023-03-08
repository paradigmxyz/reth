use std::collections::HashMap;

use crate::{
    keccak256,
    proofs::{KeccakHasher, EMPTY_ROOT},
    utils::serde_helpers::deserialize_stringified_u64,
    Address, Bytes, H256, KECCAK_EMPTY, U256,
};
use ethers_core::utils::GenesisAccount as EthersGenesisAccount;
use reth_rlp::{encode_fixed_size, length_of_length, Encodable, Header as RlpHeader};
use serde::{Deserialize, Serialize};
use triehash::sec_trie_root;

/// The genesis block specification.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
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

impl Genesis {
    /// Set the nonce.
    pub fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = nonce;
        self
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp: u64) -> Self {
        self.timestamp = timestamp;
        self
    }

    /// Set the extra data.
    pub fn with_extra_data(mut self, extra_data: Bytes) -> Self {
        self.extra_data = extra_data;
        self
    }

    /// Set the gas limit.
    pub fn with_gas_limit(mut self, gas_limit: u64) -> Self {
        self.gas_limit = gas_limit;
        self
    }

    /// Set the difficulty.
    pub fn with_difficulty(mut self, difficulty: U256) -> Self {
        self.difficulty = difficulty;
        self
    }

    /// Set the mix hash of the header.
    pub fn with_mix_hash(mut self, mix_hash: H256) -> Self {
        self.mix_hash = mix_hash;
        self
    }

    /// Set the coinbase address.
    pub fn with_coinbase(mut self, address: Address) -> Self {
        self.coinbase = address;
        self
    }

    /// Add accounts to the genesis block. If the address is already present,
    /// the account is updated.
    pub fn extend_accounts(
        mut self,
        accounts: impl IntoIterator<Item = (Address, GenesisAccount)>,
    ) -> Self {
        self.alloc.extend(accounts);
        self
    }
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
        // we are encoding a hash, so let's just use the length of the empty hash for the code hash
        len += KECCAK_EMPTY.length();
        len
    }

    /// Set the nonce.
    pub fn with_nonce(mut self, nonce: Option<u64>) -> Self {
        self.nonce = nonce;
        self
    }

    /// Set the balance.
    pub fn with_balance(mut self, balance: U256) -> Self {
        self.balance = balance;
        self
    }

    /// Set the code.
    pub fn with_code(mut self, code: Option<Bytes>) -> Self {
        self.code = code;
        self
    }

    /// Set the storage.
    pub fn with_storage(mut self, storage: Option<HashMap<H256, H256>>) -> Self {
        self.storage = storage;
        self
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
                if storage.is_empty() {
                    return EMPTY_ROOT
                }
                let storage_values =
                    storage.iter().filter(|(_k, &v)| v != H256::zero()).map(|(&k, v)| {
                        let value = U256::from_be_bytes(**v);
                        (k, encode_fixed_size(&value))
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

#[cfg(test)]
mod tests {
    use super::*;
    use hex_literal::hex;

    #[test]
    fn test_genesis() {
        let default_genesis = Genesis::default();

        let nonce = 999;
        let timestamp = 12345;
        let extra_data = Bytes::from(b"extra-data");
        let gas_limit = 333333;
        let difficulty = U256::from(9000);
        let mix_hash =
            hex!("74385b512f1e0e47100907efe2b00ac78df26acba6dd16b0772923068a5801a8").into();
        let coinbase = hex!("265873b6faf3258b3ab0827805386a2a20ed040e").into();
        // create dummy account
        let first_address: Address = hex!("7618a8c597b89e01c66a1f662078992c52a30c9a").into();
        let mut account = HashMap::default();
        account.insert(first_address, GenesisAccount::default());

        // check values updated
        let custom_genesis = Genesis::default()
            .with_nonce(nonce)
            .with_timestamp(timestamp)
            .with_extra_data(extra_data.clone())
            .with_gas_limit(gas_limit)
            .with_difficulty(difficulty)
            .with_mix_hash(mix_hash)
            .with_coinbase(coinbase)
            .extend_accounts(account.clone());

        assert_ne!(custom_genesis, default_genesis);
        // check every field
        assert_eq!(custom_genesis.nonce, nonce);
        assert_eq!(custom_genesis.timestamp, timestamp);
        assert_eq!(custom_genesis.extra_data, extra_data);
        assert_eq!(custom_genesis.gas_limit, gas_limit);
        assert_eq!(custom_genesis.difficulty, difficulty);
        assert_eq!(custom_genesis.mix_hash, mix_hash);
        assert_eq!(custom_genesis.coinbase, coinbase);
        assert_eq!(custom_genesis.alloc, account.clone());

        // update existing account
        assert_eq!(custom_genesis.alloc.len(), 1);
        let same_address = first_address;
        let new_alloc_account = GenesisAccount {
            nonce: Some(1),
            balance: U256::from(1),
            code: Some(Bytes::from(b"code")),
            storage: Some(HashMap::default()),
        };
        let mut updated_account = HashMap::default();
        updated_account.insert(same_address, new_alloc_account);
        let custom_genesis = custom_genesis.extend_accounts(updated_account.clone());
        assert_ne!(account, updated_account);
        assert_eq!(custom_genesis.alloc.len(), 1);

        // add second account
        let different_address = hex!("94e0681e3073dd71cec54b53afe988f39078fd1a").into();
        let more_accounts = HashMap::from([(different_address, GenesisAccount::default())]);
        let custom_genesis = custom_genesis.extend_accounts(more_accounts);
        assert_eq!(custom_genesis.alloc.len(), 2);

        // ensure accounts are different
        let first_account = custom_genesis.alloc.get(&first_address);
        let second_account = custom_genesis.alloc.get(&different_address);
        assert!(first_account.is_some());
        assert!(second_account.is_some());
        assert_ne!(first_account, second_account);
    }

    #[test]
    fn test_genesis_account() {
        let default_account = GenesisAccount::default();

        let nonce = Some(1);
        let balance = U256::from(33);
        let code = Some(Bytes::from(b"code"));
        let root = hex!("9474ddfcea39c5a690d2744103e39d1ff1b03d18db10fc147d970ad24699395a").into();
        let value = hex!("58eb8294d9bb16832a9dabfcb270fff99ab8ee1d8764e4f3d9fdf59ec1dee469").into();
        let mut map = HashMap::default();
        map.insert(root, value);
        let storage = Some(map);

        let genesis_account = GenesisAccount::default()
            .with_nonce(nonce)
            .with_balance(balance)
            .with_code(code.clone())
            .with_storage(storage.clone());

        assert_ne!(default_account, genesis_account);
        // check every field
        assert_eq!(genesis_account.nonce, nonce);
        assert_eq!(genesis_account.balance, balance);
        assert_eq!(genesis_account.code, code);
        assert_eq!(genesis_account.storage, storage);
    }
}
