use crate::{
    keccak256,
    proofs::{KeccakHasher, EMPTY_ROOT},
    serde_helper::{deserialize_json_u256, deserialize_json_u256_opt, deserialize_storage_map},
    utils::serde_helpers::{deserialize_stringified_u64, deserialize_stringified_u64_opt},
    Account, Address, Bytes, H256, KECCAK_EMPTY, U256,
};
use reth_rlp::{encode_fixed_size, length_of_length, Encodable, Header as RlpHeader};
use revm_primitives::B160;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use triehash::sec_trie_root;

/// The genesis block specification.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Genesis {
    /// The fork configuration for this network.
    #[serde(default)]
    pub config: ChainConfig,
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
    #[serde(deserialize_with = "deserialize_json_u256")]
    pub difficulty: U256,
    /// The genesis header mix hash.
    pub mix_hash: H256,
    /// The genesis header coinbase address.
    pub coinbase: Address,
    /// The initial state of accounts in the genesis block.
    pub alloc: HashMap<Address, GenesisAccount>,
    // NOTE: the following fields:
    // * base_fee_per_gas
    // * excess_blob_gas
    // * blob_gas_used
    // should NOT be set in a real genesis file, but are included here for compatibility with
    // consensus tests, which have genesis files with these fields populated.
    /// The genesis header base fee
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub base_fee_per_gas: Option<u64>,
    /// The genesis header excess blob gas
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub excess_blob_gas: Option<u64>,
    /// The genesis header blob gas used
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub blob_gas_used: Option<u64>,
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

    /// Set the base fee.
    pub fn with_base_fee(mut self, base_fee: Option<u64>) -> Self {
        self.base_fee_per_gas = base_fee;
        self
    }

    /// Set the excess blob gas.
    pub fn with_excess_blob_gas(mut self, excess_blob_gas: Option<u64>) -> Self {
        self.excess_blob_gas = excess_blob_gas;
        self
    }

    /// Set the blob gas used.
    pub fn with_blob_gas_used(mut self, blob_gas_used: Option<u64>) -> Self {
        self.blob_gas_used = blob_gas_used;
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
#[serde(deny_unknown_fields)]
pub struct GenesisAccount {
    /// The nonce of the account at genesis.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt",
        default
    )]
    pub nonce: Option<u64>,
    /// The balance of the account at genesis.
    #[serde(deserialize_with = "deserialize_json_u256")]
    pub balance: U256,
    /// The account's bytecode at genesis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Bytes>,
    /// The account's storage at genesis.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_storage_map"
    )]
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

    fn length(&self) -> usize {
        let len = self.payload_len();
        // RLP header length + payload length
        len + length_of_length(len)
    }
}

impl From<GenesisAccount> for Account {
    fn from(value: GenesisAccount) -> Self {
        Account {
            // nonce must exist, so we default to zero when converting a genesis account
            nonce: value.nonce.unwrap_or_default(),
            balance: value.balance,
            bytecode_hash: value.code.map(keccak256),
        }
    }
}

/// Represents a node's chain configuration.
///
/// See [geth's `ChainConfig`
/// struct](https://github.com/ethereum/go-ethereum/blob/64dccf7aa411c5c7cd36090c3d9b9892945ae813/params/config.go#L349)
/// for the source of each field.
#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ChainConfig {
    /// The network's chain ID.
    #[serde(default = "mainnet_id")]
    pub chain_id: u64,

    /// The homestead switch block (None = no fork, 0 = already homestead).
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub homestead_block: Option<u64>,

    /// The DAO fork switch block (None = no fork).
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub dao_fork_block: Option<u64>,

    /// Whether or not the node supports the DAO hard-fork.
    pub dao_fork_support: bool,

    /// The EIP-150 hard fork block (None = no fork).
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub eip150_block: Option<u64>,

    /// The EIP-150 hard fork hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eip150_hash: Option<H256>,

    /// The EIP-155 hard fork block.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub eip155_block: Option<u64>,

    /// The EIP-158 hard fork block.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub eip158_block: Option<u64>,

    /// The Byzantium hard fork block.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub byzantium_block: Option<u64>,

    /// The Constantinople hard fork block.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub constantinople_block: Option<u64>,

    /// The Petersburg hard fork block.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub petersburg_block: Option<u64>,

    /// The Istanbul hard fork block.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub istanbul_block: Option<u64>,

    /// The Muir Glacier hard fork block.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub muir_glacier_block: Option<u64>,

    /// The Berlin hard fork block.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub berlin_block: Option<u64>,

    /// The London hard fork block.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub london_block: Option<u64>,

    /// The Arrow Glacier hard fork block.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub arrow_glacier_block: Option<u64>,

    /// The Gray Glacier hard fork block.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub gray_glacier_block: Option<u64>,

    /// Virtual fork after the merge to use as a network splitter.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub merge_netsplit_block: Option<u64>,

    /// Shanghai switch time.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub shanghai_time: Option<u64>,

    /// Cancun switch time.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub cancun_time: Option<u64>,

    /// Total difficulty reached that triggers the merge consensus upgrade.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_json_u256_opt"
    )]
    pub terminal_total_difficulty: Option<U256>,

    /// A flag specifying that the network already passed the terminal total difficulty. Its
    /// purpose is to disable legacy sync without having seen the TTD locally.
    pub terminal_total_difficulty_passed: bool,

    /// Ethash parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ethash: Option<EthashConfig>,

    /// Clique parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clique: Option<CliqueConfig>,
}

// used only for serde
#[inline]
const fn mainnet_id() -> u64 {
    1
}

/// Empty consensus configuration for proof-of-work networks.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EthashConfig {}

/// Consensus configuration for Clique.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CliqueConfig {
    /// Number of seconds between blocks to enforce.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub period: Option<u64>,

    /// Epoch length to reset votes and checkpoints.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_stringified_u64_opt"
    )]
    pub epoch: Option<u64>,
}

mod ethers_compat {
    use super::*;
    use ethers_core::utils::{
        ChainConfig as EthersChainConfig, CliqueConfig as EthersCliqueConfig,
        EthashConfig as EthersEthashConfig, GenesisAccount as EthersGenesisAccount,
    };

    impl From<ethers_core::utils::Genesis> for Genesis {
        fn from(genesis: ethers_core::utils::Genesis) -> Genesis {
            let alloc = genesis
                .alloc
                .iter()
                .map(|(addr, account)| (addr.0.into(), account.clone().into()))
                .collect::<HashMap<B160, GenesisAccount>>();

            Genesis {
                config: genesis.config.into(),
                nonce: genesis.nonce.as_u64(),
                timestamp: genesis.timestamp.as_u64(),
                gas_limit: genesis.gas_limit.as_u64(),
                difficulty: genesis.difficulty.into(),
                mix_hash: genesis.mix_hash.0.into(),
                coinbase: genesis.coinbase.0.into(),
                extra_data: genesis.extra_data.0.into(),
                base_fee_per_gas: genesis.base_fee_per_gas.map(|fee| fee.as_u64()),
                // TODO: if/when ethers has cancun fields they should be added here
                excess_blob_gas: None,
                blob_gas_used: None,
                alloc,
            }
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

    impl From<EthersChainConfig> for ChainConfig {
        fn from(chain_config: EthersChainConfig) -> Self {
            let EthersChainConfig {
                chain_id,
                homestead_block,
                dao_fork_block,
                dao_fork_support,
                eip150_block,
                eip150_hash,
                eip155_block,
                eip158_block,
                byzantium_block,
                constantinople_block,
                petersburg_block,
                istanbul_block,
                muir_glacier_block,
                berlin_block,
                london_block,
                arrow_glacier_block,
                gray_glacier_block,
                merge_netsplit_block,
                shanghai_time,
                cancun_time,
                terminal_total_difficulty,
                terminal_total_difficulty_passed,
                ethash,
                clique,
            } = chain_config;

            Self {
                chain_id,
                homestead_block,
                dao_fork_block,
                dao_fork_support,
                eip150_block,
                eip150_hash: eip150_hash.map(Into::into),
                eip155_block,
                eip158_block,
                byzantium_block,
                constantinople_block,
                petersburg_block,
                istanbul_block,
                muir_glacier_block,
                berlin_block,
                london_block,
                arrow_glacier_block,
                gray_glacier_block,
                merge_netsplit_block,
                shanghai_time,
                cancun_time,
                terminal_total_difficulty: terminal_total_difficulty.map(Into::into),
                terminal_total_difficulty_passed,
                ethash: ethash.map(Into::into),
                clique: clique.map(Into::into),
            }
        }
    }

    impl From<EthersEthashConfig> for EthashConfig {
        fn from(_: EthersEthashConfig) -> Self {
            EthashConfig {}
        }
    }

    impl From<EthersCliqueConfig> for CliqueConfig {
        fn from(clique_config: EthersCliqueConfig) -> Self {
            let EthersCliqueConfig { period, epoch } = clique_config;
            Self { period, epoch }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Address, Bytes, U256};
    use hex_literal::hex;
    use std::{collections::HashMap, str::FromStr};

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

    #[test]
    fn parse_hive_genesis() {
        let geth_genesis = r#"
    {
        "difficulty": "0x20000",
        "gasLimit": "0x1",
        "alloc": {},
        "config": {
          "ethash": {},
          "chainId": 1
        }
    }
    "#;

        let _genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();
    }

    #[test]
    fn parse_hive_clique_smoke_genesis() {
        let geth_genesis = r#"
    {
      "difficulty": "0x1",
      "gasLimit": "0x400000",
      "extraData":
    "0x0000000000000000000000000000000000000000000000000000000000000000658bdf435d810c91414ec09147daa6db624063790000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
    ,   "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "nonce": "0x0",
      "timestamp": "0x5c51a607",
      "alloc": {}
    }
    "#;

        let _genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();
    }

    #[test]
    fn parse_non_hex_prefixed_balance() {
        // tests that we can parse balance / difficulty fields that are either hex or decimal
        let example_balance_json = r#"
    {
        "nonce": "0x0000000000000042",
        "difficulty": "34747478",
        "mixHash": "0x123456789abcdef123456789abcdef123456789abcdef123456789abcdef1234",
        "coinbase": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "timestamp": "0x123456",
        "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "extraData": "0xfafbfcfd",
        "gasLimit": "0x2fefd8",
        "alloc": {
            "0x3E951C9f69a06Bc3AD71fF7358DbC56bEd94b9F2": {
              "balance": "1000000000000000000000000000"
            },
            "0xe228C30d4e5245f967ac21726d5412dA27aD071C": {
              "balance": "1000000000000000000000000000"
            },
            "0xD59Ce7Ccc6454a2D2C2e06bbcf71D0Beb33480eD": {
              "balance": "1000000000000000000000000000"
            },
            "0x1CF4D54414eF51b41f9B2238c57102ab2e61D1F2": {
              "balance": "1000000000000000000000000000"
            },
            "0x249bE3fDEd872338C733cF3975af9736bdCb9D4D": {
              "balance": "1000000000000000000000000000"
            },
            "0x3fCd1bff94513712f8cD63d1eD66776A67D5F78e": {
              "balance": "1000000000000000000000000000"
            }
        },
        "config": {
            "ethash": {},
            "chainId": 10,
            "homesteadBlock": 0,
            "eip150Block": 0,
            "eip155Block": 0,
            "eip158Block": 0,
            "byzantiumBlock": 0,
            "constantinopleBlock": 0,
            "petersburgBlock": 0,
            "istanbulBlock": 0
        }
    }
    "#;

        let genesis: Genesis = serde_json::from_str(example_balance_json).unwrap();

        // check difficulty against hex ground truth
        let expected_difficulty = U256::from_str("0x2123456").unwrap();
        assert_eq!(expected_difficulty, genesis.difficulty);

        // check all alloc balances
        let dec_balance = U256::from_str("1000000000000000000000000000").unwrap();
        for alloc in &genesis.alloc {
            assert_eq!(alloc.1.balance, dec_balance);
        }
    }

    #[test]
    fn parse_hive_rpc_genesis() {
        let geth_genesis = r#"
    {
      "config": {
        "chainId": 7,
        "homesteadBlock": 0,
        "eip150Block": 0,
        "eip150Hash": "0x5de1ee4135274003348e80b788e5afa4b18b18d320a5622218d5c493fedf5689",
        "eip155Block": 0,
        "eip158Block": 0
      },
      "coinbase": "0x0000000000000000000000000000000000000000",
      "difficulty": "0x20000",
      "extraData":
    "0x0000000000000000000000000000000000000000000000000000000000000000658bdf435d810c91414ec09147daa6db624063790000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
    ,   "gasLimit": "0x2fefd8",
      "nonce": "0x0000000000000000",
      "timestamp": "0x1234",
      "alloc": {
        "cf49fda3be353c69b41ed96333cd24302da4556f": {
          "balance": "0x123450000000000000000"
        },
        "0161e041aad467a890839d5b08b138c1e6373072": {
          "balance": "0x123450000000000000000"
        },
        "87da6a8c6e9eff15d703fc2773e32f6af8dbe301": {
          "balance": "0x123450000000000000000"
        },
        "b97de4b8c857e4f6bc354f226dc3249aaee49209": {
          "balance": "0x123450000000000000000"
        },
        "c5065c9eeebe6df2c2284d046bfc906501846c51": {
          "balance": "0x123450000000000000000"
        },
        "0000000000000000000000000000000000000314": {
          "balance": "0x0",
          "code":
    "0x60606040526000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff168063a223e05d1461006a578063abd1a0cf1461008d578063abfced1d146100d4578063e05c914a14610110578063e6768b451461014c575b610000565b346100005761007761019d565b6040518082815260200191505060405180910390f35b34610000576100be600480803573ffffffffffffffffffffffffffffffffffffffff169060200190919050506101a3565b6040518082815260200191505060405180910390f35b346100005761010e600480803573ffffffffffffffffffffffffffffffffffffffff169060200190919080359060200190919050506101ed565b005b346100005761014a600480803590602001909190803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050610236565b005b346100005761017960048080359060200190919080359060200190919080359060200190919050506103c4565b60405180848152602001838152602001828152602001935050505060405180910390f35b60005481565b6000600160008373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205490505b919050565b80600160008473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055505b5050565b7f6031a8d62d7c95988fa262657cd92107d90ed96e08d8f867d32f26edfe85502260405180905060405180910390a17f47e2689743f14e97f7dcfa5eec10ba1dff02f83b3d1d4b9c07b206cbbda66450826040518082815260200191505060405180910390a1817fa48a6b249a5084126c3da369fbc9b16827ead8cb5cdc094b717d3f1dcd995e2960405180905060405180910390a27f7890603b316f3509577afd111710f9ebeefa15e12f72347d9dffd0d65ae3bade81604051808273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200191505060405180910390a18073ffffffffffffffffffffffffffffffffffffffff167f7efef9ea3f60ddc038e50cccec621f86a0195894dc0520482abf8b5c6b659e4160405180905060405180910390a28181604051808381526020018273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019250505060405180910390a05b5050565b6000600060008585859250925092505b935093509390505600a165627a7a72305820aaf842d0d0c35c45622c5263cbb54813d2974d3999c8c38551d7c613ea2bc1170029"
    ,       "storage": {
            "0x0000000000000000000000000000000000000000000000000000000000000000": "0x1234",
            "0x6661e9d6d8b923d5bbaab1b96e1dd51ff6ea2a93520fdc9eb75d059238b8c5e9": "0x01"
          }
        },
        "0000000000000000000000000000000000000315": {
          "balance": "0x9999999999999999999999999999999",
          "code":
    "0x60606040526000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff168063ef2769ca1461003e575b610000565b3461000057610078600480803573ffffffffffffffffffffffffffffffffffffffff1690602001909190803590602001909190505061007a565b005b8173ffffffffffffffffffffffffffffffffffffffff166108fc829081150290604051809050600060405180830381858888f1935050505015610106578173ffffffffffffffffffffffffffffffffffffffff167f30a3c50752f2552dcc2b93f5b96866280816a986c0c0408cb6778b9fa198288f826040518082815260200191505060405180910390a25b5b50505600a165627a7a72305820637991fabcc8abad4294bf2bb615db78fbec4edff1635a2647d3894e2daf6a610029"
        }
      }
    }
    "#;

        let _genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();
    }

    #[test]
    fn parse_hive_graphql_genesis() {
        let geth_genesis = r#"
    {
        "config"     : {},
        "coinbase"   : "0x8888f1f195afa192cfee860698584c030f4c9db1",
        "difficulty" : "0x020000",
        "extraData"  : "0x42",
        "gasLimit"   : "0x2fefd8",
        "mixHash"    : "0x2c85bcbce56429100b2108254bb56906257582aeafcbd682bc9af67a9f5aee46",
        "nonce"      : "0x78cc16f7b4f65485",
        "parentHash" : "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp"  : "0x54c98c81",
        "alloc"      : {
            "a94f5374fce5edbc8e2a8697c15331677e6ebf0b": {
                "balance" : "0x09184e72a000"
            }
        }
    }
    "#;

        let _genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();
    }

    #[test]
    fn parse_hive_engine_genesis() {
        let geth_genesis = r#"
    {
      "config": {
        "chainId": 7,
        "homesteadBlock": 0,
        "eip150Block": 0,
        "eip150Hash": "0x5de1ee4135274003348e80b788e5afa4b18b18d320a5622218d5c493fedf5689",
        "eip155Block": 0,
        "eip158Block": 0,
        "byzantiumBlock": 0,
        "constantinopleBlock": 0,
        "petersburgBlock": 0,
        "istanbulBlock": 0,
        "muirGlacierBlock": 0,
        "berlinBlock": 0,
        "yolov2Block": 0,
        "yolov3Block": 0,
        "londonBlock": 0
      },
      "coinbase": "0x0000000000000000000000000000000000000000",
      "difficulty": "0x30000",
      "extraData":
    "0x0000000000000000000000000000000000000000000000000000000000000000658bdf435d810c91414ec09147daa6db624063790000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
    ,   "gasLimit": "0x2fefd8",
      "nonce": "0x0000000000000000",
      "timestamp": "0x1234",
      "alloc": {
        "cf49fda3be353c69b41ed96333cd24302da4556f": {
          "balance": "0x123450000000000000000"
        },
        "0161e041aad467a890839d5b08b138c1e6373072": {
          "balance": "0x123450000000000000000"
        },
        "87da6a8c6e9eff15d703fc2773e32f6af8dbe301": {
          "balance": "0x123450000000000000000"
        },
        "b97de4b8c857e4f6bc354f226dc3249aaee49209": {
          "balance": "0x123450000000000000000"
        },
        "c5065c9eeebe6df2c2284d046bfc906501846c51": {
          "balance": "0x123450000000000000000"
        },
        "0000000000000000000000000000000000000314": {
          "balance": "0x0",
          "code":
    "0x60606040526000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff168063a223e05d1461006a578063abd1a0cf1461008d578063abfced1d146100d4578063e05c914a14610110578063e6768b451461014c575b610000565b346100005761007761019d565b6040518082815260200191505060405180910390f35b34610000576100be600480803573ffffffffffffffffffffffffffffffffffffffff169060200190919050506101a3565b6040518082815260200191505060405180910390f35b346100005761010e600480803573ffffffffffffffffffffffffffffffffffffffff169060200190919080359060200190919050506101ed565b005b346100005761014a600480803590602001909190803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050610236565b005b346100005761017960048080359060200190919080359060200190919080359060200190919050506103c4565b60405180848152602001838152602001828152602001935050505060405180910390f35b60005481565b6000600160008373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205490505b919050565b80600160008473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055505b5050565b7f6031a8d62d7c95988fa262657cd92107d90ed96e08d8f867d32f26edfe85502260405180905060405180910390a17f47e2689743f14e97f7dcfa5eec10ba1dff02f83b3d1d4b9c07b206cbbda66450826040518082815260200191505060405180910390a1817fa48a6b249a5084126c3da369fbc9b16827ead8cb5cdc094b717d3f1dcd995e2960405180905060405180910390a27f7890603b316f3509577afd111710f9ebeefa15e12f72347d9dffd0d65ae3bade81604051808273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200191505060405180910390a18073ffffffffffffffffffffffffffffffffffffffff167f7efef9ea3f60ddc038e50cccec621f86a0195894dc0520482abf8b5c6b659e4160405180905060405180910390a28181604051808381526020018273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019250505060405180910390a05b5050565b6000600060008585859250925092505b935093509390505600a165627a7a72305820aaf842d0d0c35c45622c5263cbb54813d2974d3999c8c38551d7c613ea2bc1170029"
    ,       "storage": {
            "0x0000000000000000000000000000000000000000000000000000000000000000": "0x1234",
            "0x6661e9d6d8b923d5bbaab1b96e1dd51ff6ea2a93520fdc9eb75d059238b8c5e9": "0x01"
          }
        },
        "0000000000000000000000000000000000000315": {
          "balance": "0x9999999999999999999999999999999",
          "code":
    "0x60606040526000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff168063ef2769ca1461003e575b610000565b3461000057610078600480803573ffffffffffffffffffffffffffffffffffffffff1690602001909190803590602001909190505061007a565b005b8173ffffffffffffffffffffffffffffffffffffffff166108fc829081150290604051809050600060405180830381858888f1935050505015610106578173ffffffffffffffffffffffffffffffffffffffff167f30a3c50752f2552dcc2b93f5b96866280816a986c0c0408cb6778b9fa198288f826040518082815260200191505060405180910390a25b5b50505600a165627a7a72305820637991fabcc8abad4294bf2bb615db78fbec4edff1635a2647d3894e2daf6a610029"
        },
        "0000000000000000000000000000000000000316": {
          "balance": "0x0",
          "code": "0x444355"
        },
        "0000000000000000000000000000000000000317": {
          "balance": "0x0",
          "code": "0x600160003555"
        }
      }
    }
    "#;

        let _genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();
    }

    #[test]
    fn parse_hive_devp2p_genesis() {
        let geth_genesis = r#"
    {
        "config": {
            "chainId": 19763,
            "homesteadBlock": 0,
            "eip150Block": 0,
            "eip155Block": 0,
            "eip158Block": 0,
            "byzantiumBlock": 0,
            "ethash": {}
        },
        "nonce": "0xdeadbeefdeadbeef",
        "timestamp": "0x0",
        "extraData": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "gasLimit": "0x80000000",
        "difficulty": "0x20000",
        "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "coinbase": "0x0000000000000000000000000000000000000000",
        "alloc": {
            "71562b71999873db5b286df957af199ec94617f7": {
                "balance": "0xffffffffffffffffffffffffff"
            }
        },
        "number": "0x0",
        "gasUsed": "0x0",
        "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000"
    }
    "#;

        let _genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();
    }

    #[test]
    fn parse_execution_apis_genesis() {
        let geth_genesis = r#"
    {
      "config": {
        "chainId": 1337,
        "homesteadBlock": 0,
        "eip150Block": 0,
        "eip150Hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "eip155Block": 0,
        "eip158Block": 0,
        "byzantiumBlock": 0,
        "constantinopleBlock": 0,
        "petersburgBlock": 0,
        "istanbulBlock": 0,
        "muirGlacierBlock": 0,
        "berlinBlock": 0,
        "londonBlock": 0,
        "arrowGlacierBlock": 0,
        "grayGlacierBlock": 0,
        "shanghaiTime": 0,
        "terminalTotalDifficulty": 0,
        "terminalTotalDifficultyPassed": true,
        "ethash": {}
      },
      "nonce": "0x0",
      "timestamp": "0x0",
      "extraData": "0x",
      "gasLimit": "0x4c4b40",
      "difficulty": "0x1",
      "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "coinbase": "0x0000000000000000000000000000000000000000",
      "alloc": {
        "658bdf435d810c91414ec09147daa6db62406379": {
          "balance": "0x487a9a304539440000"
        },
        "aa00000000000000000000000000000000000000": {
          "code": "0x6042",
          "storage": {
            "0x0000000000000000000000000000000000000000000000000000000000000000":
    "0x0000000000000000000000000000000000000000000000000000000000000000",
            "0x0100000000000000000000000000000000000000000000000000000000000000":
    "0x0100000000000000000000000000000000000000000000000000000000000000",
            "0x0200000000000000000000000000000000000000000000000000000000000000":
    "0x0200000000000000000000000000000000000000000000000000000000000000",
            "0x0300000000000000000000000000000000000000000000000000000000000000":
    "0x0000000000000000000000000000000000000000000000000000000000000303"       },
          "balance": "0x1",
          "nonce": "0x1"
        },
        "bb00000000000000000000000000000000000000": {
          "code": "0x600154600354",
          "storage": {
            "0x0000000000000000000000000000000000000000000000000000000000000000":
    "0x0000000000000000000000000000000000000000000000000000000000000000",
            "0x0100000000000000000000000000000000000000000000000000000000000000":
    "0x0100000000000000000000000000000000000000000000000000000000000000",
            "0x0200000000000000000000000000000000000000000000000000000000000000":
    "0x0200000000000000000000000000000000000000000000000000000000000000",
            "0x0300000000000000000000000000000000000000000000000000000000000000":
    "0x0000000000000000000000000000000000000000000000000000000000000303"       },
          "balance": "0x2",
          "nonce": "0x1"
        }
      }
    }
    "#;

        let _genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();
    }

    #[test]
    fn parse_hive_rpc_genesis_full() {
        let geth_genesis = r#"
    {
      "config": {
        "clique": {
          "period": 1
        },
        "chainId": 7,
        "homesteadBlock": 0,
        "eip150Block": 0,
        "eip155Block": 0,
        "eip158Block": 0
      },
      "coinbase": "0x0000000000000000000000000000000000000000",
      "difficulty": "0x020000",
      "extraData":
    "0x0000000000000000000000000000000000000000000000000000000000000000658bdf435d810c91414ec09147daa6db624063790000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
    ,   "gasLimit": "0x2fefd8",
      "nonce": "0x0000000000000000",
      "timestamp": "0x1234",
      "alloc": {
        "cf49fda3be353c69b41ed96333cd24302da4556f": {
          "balance": "0x123450000000000000000"
        },
        "0161e041aad467a890839d5b08b138c1e6373072": {
          "balance": "0x123450000000000000000"
        },
        "87da6a8c6e9eff15d703fc2773e32f6af8dbe301": {
          "balance": "0x123450000000000000000"
        },
        "b97de4b8c857e4f6bc354f226dc3249aaee49209": {
          "balance": "0x123450000000000000000"
        },
        "c5065c9eeebe6df2c2284d046bfc906501846c51": {
          "balance": "0x123450000000000000000"
        },
        "0000000000000000000000000000000000000314": {
          "balance": "0x0",
          "code":
    "0x60606040526000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff168063a223e05d1461006a578063abd1a0cf1461008d578063abfced1d146100d4578063e05c914a14610110578063e6768b451461014c575b610000565b346100005761007761019d565b6040518082815260200191505060405180910390f35b34610000576100be600480803573ffffffffffffffffffffffffffffffffffffffff169060200190919050506101a3565b6040518082815260200191505060405180910390f35b346100005761010e600480803573ffffffffffffffffffffffffffffffffffffffff169060200190919080359060200190919050506101ed565b005b346100005761014a600480803590602001909190803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050610236565b005b346100005761017960048080359060200190919080359060200190919080359060200190919050506103c4565b60405180848152602001838152602001828152602001935050505060405180910390f35b60005481565b6000600160008373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205490505b919050565b80600160008473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055505b5050565b7f6031a8d62d7c95988fa262657cd92107d90ed96e08d8f867d32f26edfe85502260405180905060405180910390a17f47e2689743f14e97f7dcfa5eec10ba1dff02f83b3d1d4b9c07b206cbbda66450826040518082815260200191505060405180910390a1817fa48a6b249a5084126c3da369fbc9b16827ead8cb5cdc094b717d3f1dcd995e2960405180905060405180910390a27f7890603b316f3509577afd111710f9ebeefa15e12f72347d9dffd0d65ae3bade81604051808273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200191505060405180910390a18073ffffffffffffffffffffffffffffffffffffffff167f7efef9ea3f60ddc038e50cccec621f86a0195894dc0520482abf8b5c6b659e4160405180905060405180910390a28181604051808381526020018273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019250505060405180910390a05b5050565b6000600060008585859250925092505b935093509390505600a165627a7a72305820aaf842d0d0c35c45622c5263cbb54813d2974d3999c8c38551d7c613ea2bc1170029"
    ,       "storage": {
            "0x0000000000000000000000000000000000000000000000000000000000000000": "0x1234",
            "0x6661e9d6d8b923d5bbaab1b96e1dd51ff6ea2a93520fdc9eb75d059238b8c5e9": "0x01"
          }
        },
        "0000000000000000000000000000000000000315": {
          "balance": "0x9999999999999999999999999999999",
          "code":
    "0x60606040526000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff168063ef2769ca1461003e575b610000565b3461000057610078600480803573ffffffffffffffffffffffffffffffffffffffff1690602001909190803590602001909190505061007a565b005b8173ffffffffffffffffffffffffffffffffffffffff166108fc829081150290604051809050600060405180830381858888f1935050505015610106578173ffffffffffffffffffffffffffffffffffffffff167f30a3c50752f2552dcc2b93f5b96866280816a986c0c0408cb6778b9fa198288f826040518082815260200191505060405180910390a25b5b50505600a165627a7a72305820637991fabcc8abad4294bf2bb615db78fbec4edff1635a2647d3894e2daf6a610029"
        }
      },
      "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000"
    }
    "#;

        let genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();
        let alloc_entry = genesis
            .alloc
            .get(&Address::from_str("0000000000000000000000000000000000000314").unwrap())
            .expect("missing account for parsed genesis");
        let storage = alloc_entry.storage.as_ref().expect("missing storage for parsed genesis");
        let expected_storage = HashMap::from_iter(vec![
            (
                H256::from_str(
                    "0x0000000000000000000000000000000000000000000000000000000000000000",
                )
                .unwrap(),
                H256::from_str(
                    "0x0000000000000000000000000000000000000000000000000000000000001234",
                )
                .unwrap(),
            ),
            (
                H256::from_str(
                    "0x6661e9d6d8b923d5bbaab1b96e1dd51ff6ea2a93520fdc9eb75d059238b8c5e9",
                )
                .unwrap(),
                H256::from_str(
                    "0x0000000000000000000000000000000000000000000000000000000000000001",
                )
                .unwrap(),
            ),
        ]);
        assert_eq!(storage, &expected_storage);

        let expected_code =
    Bytes::from_str("0x60606040526000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff168063a223e05d1461006a578063abd1a0cf1461008d578063abfced1d146100d4578063e05c914a14610110578063e6768b451461014c575b610000565b346100005761007761019d565b6040518082815260200191505060405180910390f35b34610000576100be600480803573ffffffffffffffffffffffffffffffffffffffff169060200190919050506101a3565b6040518082815260200191505060405180910390f35b346100005761010e600480803573ffffffffffffffffffffffffffffffffffffffff169060200190919080359060200190919050506101ed565b005b346100005761014a600480803590602001909190803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050610236565b005b346100005761017960048080359060200190919080359060200190919080359060200190919050506103c4565b60405180848152602001838152602001828152602001935050505060405180910390f35b60005481565b6000600160008373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205490505b919050565b80600160008473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055505b5050565b7f6031a8d62d7c95988fa262657cd92107d90ed96e08d8f867d32f26edfe85502260405180905060405180910390a17f47e2689743f14e97f7dcfa5eec10ba1dff02f83b3d1d4b9c07b206cbbda66450826040518082815260200191505060405180910390a1817fa48a6b249a5084126c3da369fbc9b16827ead8cb5cdc094b717d3f1dcd995e2960405180905060405180910390a27f7890603b316f3509577afd111710f9ebeefa15e12f72347d9dffd0d65ae3bade81604051808273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200191505060405180910390a18073ffffffffffffffffffffffffffffffffffffffff167f7efef9ea3f60ddc038e50cccec621f86a0195894dc0520482abf8b5c6b659e4160405180905060405180910390a28181604051808381526020018273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019250505060405180910390a05b5050565b6000600060008585859250925092505b935093509390505600a165627a7a72305820aaf842d0d0c35c45622c5263cbb54813d2974d3999c8c38551d7c613ea2bc1170029"
    ).unwrap();
        let code = alloc_entry.code.as_ref().expect(
            "missing code for parsed
    genesis",
        );
        assert_eq!(code, &expected_code);
    }

    #[test]
    fn test_hive_smoke_alloc_deserialize() {
        let hive_genesis = r#"
    {
        "nonce": "0x0000000000000042",
        "difficulty": "0x2123456",
        "mixHash": "0x123456789abcdef123456789abcdef123456789abcdef123456789abcdef1234",
        "coinbase": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "timestamp": "0x123456",
        "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "extraData": "0xfafbfcfd",
        "gasLimit": "0x2fefd8",
        "alloc": {
            "dbdbdb2cbd23b783741e8d7fcf51e459b497e4a6": {
                "balance": "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            },
            "e6716f9544a56c530d868e4bfbacb172315bdead": {
                "balance": "0x11",
                "code": "0x12"
            },
            "b9c015918bdaba24b4ff057a92a3873d6eb201be": {
                "balance": "0x21",
                "storage": {
                    "0x0000000000000000000000000000000000000000000000000000000000000001": "0x22"
                }
            },
            "1a26338f0d905e295fccb71fa9ea849ffa12aaf4": {
                "balance": "0x31",
                "nonce": "0x32"
            },
            "0000000000000000000000000000000000000001": {
                "balance": "0x41"
            },
            "0000000000000000000000000000000000000002": {
                "balance": "0x51"
            },
            "0000000000000000000000000000000000000003": {
                "balance": "0x61"
            },
            "0000000000000000000000000000000000000004": {
                "balance": "0x71"
            }
        },
        "config": {
            "ethash": {},
            "chainId": 10,
            "homesteadBlock": 0,
            "eip150Block": 0,
            "eip155Block": 0,
            "eip158Block": 0,
            "byzantiumBlock": 0,
            "constantinopleBlock": 0,
            "petersburgBlock": 0,
            "istanbulBlock": 0
        }
    }
    "#;

        let expected_genesis =
            Genesis {
                nonce: 0x0000000000000042,
                difficulty: U256::from(0x2123456),
                mix_hash: H256::from_str(
                    "0x123456789abcdef123456789abcdef123456789abcdef123456789abcdef1234",
                )
                .unwrap(),
                coinbase: Address::from_str("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
                timestamp: 0x123456,
                extra_data: Bytes::from_str("0xfafbfcfd").unwrap(),
                gas_limit: 0x2fefd8,
                base_fee_per_gas: None,
                excess_blob_gas: None,
                blob_gas_used: None,
                alloc: HashMap::from_iter(vec![
                (
                    Address::from_str("0xdbdbdb2cbd23b783741e8d7fcf51e459b497e4a6").unwrap(),
                    GenesisAccount {
                        balance:
    U256::from_str("0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff").
    unwrap(),                     nonce: None,
                        code: None,
                        storage: None,
                    },
                ),
                (
                    Address::from_str("0xe6716f9544a56c530d868e4bfbacb172315bdead").unwrap(),
                    GenesisAccount {
                        balance: U256::from_str("0x11").unwrap(),
                        nonce: None,
                        code: Some(Bytes::from_str("0x12").unwrap()),
                        storage: None,
                    },
                ),
                (
                    Address::from_str("0xb9c015918bdaba24b4ff057a92a3873d6eb201be").unwrap(),
                    GenesisAccount {
                        balance: U256::from_str("0x21").unwrap(),
                        nonce: None,
                        code: None,
                        storage: Some(HashMap::from_iter(vec![
                            (

    H256::from_str("0x0000000000000000000000000000000000000000000000000000000000000001").
    unwrap(),
    H256::from_str("0x0000000000000000000000000000000000000000000000000000000000000022").
    unwrap(),                         ),
                        ])),
                    },
                ),
                (
                    Address::from_str("0x1a26338f0d905e295fccb71fa9ea849ffa12aaf4").unwrap(),
                    GenesisAccount {
                        balance: U256::from_str("0x31").unwrap(),
                        nonce: Some(0x32u64),
                        code: None,
                        storage: None,
                    },
                ),
                (
                    Address::from_str("0x0000000000000000000000000000000000000001").unwrap(),
                    GenesisAccount {
                        balance: U256::from_str("0x41").unwrap(),
                        nonce: None,
                        code: None,
                        storage: None,
                    },
                ),
                (
                    Address::from_str("0x0000000000000000000000000000000000000002").unwrap(),
                    GenesisAccount {
                        balance: U256::from_str("0x51").unwrap(),
                        nonce: None,
                        code: None,
                        storage: None,
                    },
                ),
                (
                    Address::from_str("0x0000000000000000000000000000000000000003").unwrap(),
                    GenesisAccount {
                        balance: U256::from_str("0x61").unwrap(),
                        nonce: None,
                        code: None,
                        storage: None,
                    },
                ),
                (
                    Address::from_str("0x0000000000000000000000000000000000000004").unwrap(),
                    GenesisAccount {
                        balance: U256::from_str("0x71").unwrap(),
                        nonce: None,
                        code: None,
                        storage: None,
                    },
                ),
            ]),
                config: ChainConfig {
                    ethash: Some(EthashConfig {}),
                    chain_id: 10,
                    homestead_block: Some(0),
                    eip150_block: Some(0),
                    eip155_block: Some(0),
                    eip158_block: Some(0),
                    byzantium_block: Some(0),
                    constantinople_block: Some(0),
                    petersburg_block: Some(0),
                    istanbul_block: Some(0),
                    ..Default::default()
                },
            };

        let deserialized_genesis: Genesis = serde_json::from_str(hive_genesis).unwrap();
        assert_eq!(
            deserialized_genesis, expected_genesis,
            "deserialized genesis
    {deserialized_genesis:#?} does not match expected {expected_genesis:#?}"
        );
    }
}
