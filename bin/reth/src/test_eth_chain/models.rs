use reth_primitives::{
    Address, BigEndianHash, Bloom, Bytes, ChainSpec, ChainSpecBuilder, Header as RethHeader,
    JsonU256, SealedHeader, H160, H256, H64, U256, U64,
};
use serde::{self, Deserialize};
use std::collections::BTreeMap;

/// An Ethereum blockchain test.
#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct Test(pub BTreeMap<String, BlockchainTestData>);

/// Ethereum test data.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockchainTestData {
    /// Genesis block header.
    pub genesis_block_header: Header,
    /// RLP encoded genesis block.
    #[serde(rename = "genesisRLP")]
    pub genesis_rlp: Option<Bytes>,
    /// Block data.
    pub blocks: Vec<Block>,
    /// The expected post state.
    pub post_state: Option<RootOrState>,
    /// The test pre-state.
    pub pre: State,
    /// Hash of the best block.
    pub lastblockhash: H256,
    /// Network spec.
    pub network: ForkSpec,
    #[serde(default)]
    /// Engine spec.
    pub self_engine: SealEngine,
}

/// A block header in an Ethereum blockchain test.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    /// Bloom filter.
    pub bloom: Bloom,
    /// Coinbase.
    pub coinbase: Address,
    /// Difficulty.
    pub difficulty: JsonU256,
    /// Extra data.
    pub extra_data: Bytes,
    /// Gas limit.
    pub gas_limit: JsonU256,
    /// Gas used.
    pub gas_used: JsonU256,
    /// Block Hash.
    pub hash: H256,
    /// Mix hash.
    pub mix_hash: H256,
    /// Seal nonce.
    pub nonce: H64,
    /// Block number.
    pub number: JsonU256,
    /// Parent hash.
    pub parent_hash: H256,
    /// Receipt trie.
    pub receipt_trie: H256,
    /// State root.
    pub state_root: H256,
    /// Timestamp.
    pub timestamp: JsonU256,
    /// Transactions trie.
    pub transactions_trie: H256,
    /// Uncle hash.
    pub uncle_hash: H256,
    /// Base fee per gas.
    pub base_fee_per_gas: Option<JsonU256>,
}

impl From<Header> for SealedHeader {
    fn from(value: Header) -> Self {
        SealedHeader::new(
            RethHeader {
                base_fee_per_gas: value.base_fee_per_gas.map(|v| v.0.to::<u64>()),
                beneficiary: value.coinbase,
                difficulty: value.difficulty.0,
                extra_data: value.extra_data,
                gas_limit: value.gas_limit.0.to::<u64>(),
                gas_used: value.gas_used.0.to::<u64>(),
                mix_hash: value.mix_hash,
                nonce: value.nonce.into_uint().as_u64(),
                number: value.number.0.to::<u64>(),
                timestamp: value.timestamp.0.to::<u64>(),
                transactions_root: value.transactions_trie,
                receipts_root: value.receipt_trie,
                ommers_hash: value.uncle_hash,
                state_root: value.state_root,
                parent_hash: value.parent_hash,
                logs_bloom: Bloom::default(), // TODO: ?
            },
            value.hash,
        )
    }
}

/// A block in an Ethereum blockchain test.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    /// Block header.
    pub block_header: Option<Header>,
    /// RLP encoded block bytes
    pub rlp: Bytes,
    /// Transactions
    pub transactions: Option<Vec<Transaction>>,
    /// Uncle/ommer headers
    pub uncle_headers: Option<Vec<Header>>,
    /// Transaction Sequence
    pub transaction_sequence: Option<Vec<TransactionSequence>>,
    /// Withdrawals
    pub withdrawals: Option<Vec<Withdrawal>>,
}

/// Transaction Sequence in block
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct TransactionSequence {
    exception: String,
    raw_bytes: Bytes,
    valid: String,
}

/// Withdrawal in block
#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Withdrawal {
    index: U64,
    validator_index: U64,
    address: Address,
    amount: U256,
}

/// Ethereum blockchain test data state.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct State(pub BTreeMap<Address, Account>);

/// Merkle root hash or storage accounts.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum RootOrState {
    /// If state is too big, only state root is present
    Root(H256),
    /// State
    State(BTreeMap<Address, Account>),
}

/// An account.
#[derive(Debug, PartialEq, Eq, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Account {
    /// Balance.
    pub balance: JsonU256,
    /// Code.
    pub code: Bytes,
    /// Nonce.
    pub nonce: JsonU256,
    /// Storage.
    pub storage: BTreeMap<JsonU256, JsonU256>,
}

/// Fork specification.
#[derive(Debug, PartialEq, Eq, PartialOrd, Hash, Ord, Deserialize)]
pub enum ForkSpec {
    /// Frontier
    Frontier,
    /// Frontier to Homestead
    FrontierToHomesteadAt5,
    /// Homestead
    Homestead,
    /// Homestead to Tangerine
    HomesteadToDaoAt5,
    /// Homestead to Tangerine
    HomesteadToEIP150At5,
    /// Tangerine
    EIP150,
    /// Spurious Dragon
    EIP158, // EIP-161: State trie clearing
    /// Spurious Dragon to Byzantium
    EIP158ToByzantiumAt5,
    /// Byzantium
    Byzantium,
    /// Byzantium to Constantinople
    ByzantiumToConstantinopleAt5, // SKIPPED
    /// Byzantium to Constantinople
    ByzantiumToConstantinopleFixAt5,
    /// Constantinople
    Constantinople, // SKIPPED
    /// Constantinople fix
    ConstantinopleFix,
    /// Istanbul
    Istanbul,
    /// Berlin
    Berlin,
    /// Berlin to London
    BerlinToLondonAt5,
    /// London
    London,
    /// Paris aka The Merge
    Merge,
    /// Shanghai
    Shanghai,
    /// Merge EOF test
    #[serde(alias = "Merge+3540+3670")]
    MergeEOF,
    /// After Merge Init Code test
    #[serde(alias = "Merge+3860")]
    MergeMeterInitCode,
    /// After Merge plus new PUSH0 opcode
    #[serde(alias = "Merge+3855")]
    MergePush0,
    /// Fork Spec which is unknown to us
    #[serde(other)]
    Unknown,
}

impl From<ForkSpec> for ChainSpec {
    fn from(fork_spec: ForkSpec) -> Self {
        let spec_builder = ChainSpecBuilder::mainnet();

        match fork_spec {
            ForkSpec::Frontier => spec_builder.frontier_activated(),
            ForkSpec::Homestead | ForkSpec::FrontierToHomesteadAt5 => {
                spec_builder.homestead_activated()
            }
            ForkSpec::EIP150 | ForkSpec::HomesteadToDaoAt5 | ForkSpec::HomesteadToEIP150At5 => {
                spec_builder.tangerine_whistle_activated()
            }
            ForkSpec::EIP158 => spec_builder.spurious_dragon_activated(),
            ForkSpec::Byzantium |
            ForkSpec::EIP158ToByzantiumAt5 |
            ForkSpec::ConstantinopleFix |
            ForkSpec::ByzantiumToConstantinopleFixAt5 => spec_builder.byzantium_activated(),
            ForkSpec::Istanbul => spec_builder.istanbul_activated(),
            ForkSpec::Berlin => spec_builder.berlin_activated(),
            ForkSpec::London | ForkSpec::BerlinToLondonAt5 => spec_builder.london_activated(),
            ForkSpec::Merge => spec_builder.paris_activated(),
            ForkSpec::MergeEOF => spec_builder.paris_activated(),
            ForkSpec::MergeMeterInitCode => spec_builder.paris_activated(),
            ForkSpec::MergePush0 => spec_builder.paris_activated(),
            ForkSpec::Shanghai => {
                panic!("Not supported")
            }
            ForkSpec::ByzantiumToConstantinopleAt5 | ForkSpec::Constantinople => {
                panic!("Overridden with PETERSBURG")
            }
            ForkSpec::Unknown => {
                panic!("Unknown fork");
            }
        }
        .build()
    }
}

/// Possible seal engines.
#[derive(Debug, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SealEngine {
    /// No consensus checks.
    #[default]
    NoProof,
}

/// Ethereum blockchain test transaction data.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    /// Transaction type
    #[serde(rename = "type")]
    pub transaction_type: Option<JsonU256>,
    /// Data.
    pub data: Bytes,
    /// Gas limit.
    pub gas_limit: JsonU256,
    /// Gas price.
    pub gas_price: Option<JsonU256>,
    /// Nonce.
    pub nonce: JsonU256,
    /// Signature r part.
    pub r: JsonU256,
    /// Signature s part.
    pub s: JsonU256,
    /// Parity bit.
    pub v: JsonU256,
    /// Transaction value.
    pub value: JsonU256,
    /// Chain ID.
    pub chain_id: Option<JsonU256>,
    /// Access list.
    pub access_list: Option<AccessList>,
    /// Max fee per gas.
    pub max_fee_per_gas: Option<JsonU256>,
    /// Max priority fee per gas
    pub max_priority_fee_per_gas: Option<JsonU256>,
    /// Transaction hash.
    pub hash: Option<H256>,
}

/// Access list item
#[derive(Debug, PartialEq, Eq, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccessListItem {
    /// Account address
    pub address: H160,
    /// Storage key.
    pub storage_keys: Vec<H256>,
}

/// Access list.
pub type AccessList = Vec<AccessListItem>;

#[cfg(test)]
mod test {
    use super::*;
    use serde_json;

    #[test]
    fn blockchain_test_deserialize() {
        let test = r#"{
            "evmBytecode_d0g0v0_Berlin" : {
                "_info" : {
                    "comment" : "",
                    "filling-rpc-server" : "evm version 1.10.18-unstable-53304ff6-20220503",
                    "filling-tool-version" : "retesteth-0.2.2-testinfo+commit.05e0b8ca.Linux.g++",
                    "generatedTestHash" : "0951de8d9e6b2a08e57234f57ef719a17aee9d7e9d7e852e454a641028b791a9",
                    "lllcversion" : "Version: 0.5.14-develop.2021.11.27+commit.401d5358.Linux.g++",
                    "solidity" : "Version: 0.8.5+commit.a4f2e591.Linux.g++",
                    "source" : "src/GeneralStateTestsFiller/stBugs/evmBytecodeFiller.json",
                    "sourceHash" : "6ced7b43100305d1cc5aee48344c0eab6002940358e2c126279ef8444c2dea5a"
                },
                "blocks" : [
                    {
                        "blockHeader" : {
                            "bloom" : "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
                            "coinbase" : "0x1000000000000000000000000000000000000000",
                            "difficulty" : "0x020000",
                            "extraData" : "0x00",
                            "gasLimit" : "0x54a60a4202e088",
                            "gasUsed" : "0x01d4c0",
                            "hash" : "0xb8152a06d2018ed9a1eb1eb6e1fe8c3478d8eae5b04d743bf4a1ec699510cfe5",
                            "mixHash" : "0x0000000000000000000000000000000000000000000000000000000000000000",
                            "nonce" : "0x0000000000000000",
                            "number" : "0x01",
                            "parentHash" : "0xb835c89a42605cfcc542381145b83c826caf10823b81af0f45091040a67e6601",
                            "receiptTrie" : "0x0ef77336cf7bfbd2c500dcefe7b48d0ef7896d38f6373fbeb301ea4dac3746a7",
                            "stateRoot" : "0x27bf1aca92967ecd83e11c52887203bbdcab73a27fe07e814cf749fa50483a53",
                            "timestamp" : "0x03e8",
                            "transactionsTrie" : "0x9008a2d4af552fea9b45675cd2af6d4117303b57da25b28438ccd1f6bad6828d",
                            "uncleHash" : "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"
                        },
                        "rlp" : "0xf90264f901fca0b835c89a42605cfcc542381145b83c826caf10823b81af0f45091040a67e6601a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347941000000000000000000000000000000000000000a027bf1aca92967ecd83e11c52887203bbdcab73a27fe07e814cf749fa50483a53a09008a2d4af552fea9b45675cd2af6d4117303b57da25b28438ccd1f6bad6828da00ef77336cf7bfbd2c500dcefe7b48d0ef7896d38f6373fbeb301ea4dac3746a7b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018754a60a4202e0888301d4c08203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f862f860800a8301d4c094b94f5374fce5edbc8e2a8697c15331677e6ebf0b80801ca0f3b41c283c02ed98318dc9cac3f0ddc3de2f2f7853a03299a46f22a7c6726c3aa0396ccb5968a532ea070924408625d3e36d4db21bfbd0cb070ba9e1fe9dba58abc0",
                        "transactions" : [
                            {
                                "data" : "0x",
                                "gasLimit" : "0x01d4c0",
                                "gasPrice" : "0x0a",
                                "nonce" : "0x00",
                                "r" : "0xf3b41c283c02ed98318dc9cac3f0ddc3de2f2f7853a03299a46f22a7c6726c3a",
                                "s" : "0x396ccb5968a532ea070924408625d3e36d4db21bfbd0cb070ba9e1fe9dba58ab",
                                "sender" : "0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b",
                                "to" : "0xb94f5374fce5edbc8e2a8697c15331677e6ebf0b",
                                "v" : "0x1c",
                                "value" : "0x00"
                            }
                        ],
                        "uncleHeaders" : [
                        ],
                        "withdrawals" : [
                        ]
                    }
                ],
                "genesisBlockHeader" : {
                    "bloom" : "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
                    "coinbase" : "0x1000000000000000000000000000000000000000",
                    "difficulty" : "0x020000",
                    "extraData" : "0x00",
                    "gasLimit" : "0x54a60a4202e088",
                    "gasUsed" : "0x00",
                    "hash" : "0xb835c89a42605cfcc542381145b83c826caf10823b81af0f45091040a67e6601",
                    "mixHash" : "0x0000000000000000000000000000000000000000000000000000000000000000",
                    "nonce" : "0x0000000000000000",
                    "number" : "0x00",
                    "parentHash" : "0x0000000000000000000000000000000000000000000000000000000000000000",
                    "receiptTrie" : "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
                    "stateRoot" : "0x642a369a4a9dbf57d83ba05413910a5dd2cff93858c68e9e8293a8fffeae8660",
                    "timestamp" : "0x00",
                    "transactionsTrie" : "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
                    "uncleHash" : "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"
                },
                "genesisRLP" : "0xf901fcf901f7a00000000000000000000000000000000000000000000000000000000000000000a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347941000000000000000000000000000000000000000a0642a369a4a9dbf57d83ba05413910a5dd2cff93858c68e9e8293a8fffeae8660a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000808754a60a4202e088808000a00000000000000000000000000000000000000000000000000000000000000000880000000000000000c0c0",
                "lastblockhash" : "0xb8152a06d2018ed9a1eb1eb6e1fe8c3478d8eae5b04d743bf4a1ec699510cfe5",
                "network" : "Berlin",
                "postState" : {
                    "0x1000000000000000000000000000000000000000" : {
                        "balance" : "0x1bc16d674eda4f80",
                        "code" : "0x",
                        "nonce" : "0x00",
                        "storage" : {
                        }
                    },
                    "0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b" : {
                        "balance" : "0x38beec8feeb7d618",
                        "code" : "0x",
                        "nonce" : "0x01",
                        "storage" : {
                        }
                    },
                    "0xb94f5374fce5edbc8e2a8697c15331677e6ebf0b" : {
                        "balance" : "0x00",
                        "code" : "0x67ffffffffffffffff600160006000fb",
                        "nonce" : "0x3f",
                        "storage" : {
                        }
                    }
                },
                "pre" : {
                    "0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b" : {
                        "balance" : "0x38beec8feeca2598",
                        "code" : "0x",
                        "nonce" : "0x00",
                        "storage" : {
                        }
                    },
                    "0xb94f5374fce5edbc8e2a8697c15331677e6ebf0b" : {
                        "balance" : "0x00",
                        "code" : "0x67ffffffffffffffff600160006000fb",
                        "nonce" : "0x3f",
                        "storage" : {
                        }
                    }
                },
                "sealEngine" : "NoProof"
            }
        }"#;

        let res = serde_json::from_str::<Test>(test);
        assert!(res.is_ok(), "Failed to deserialize BlockchainTestData with error: {res:?}");
    }

    #[test]
    fn header_deserialize() {
        let test = r#"{
            "baseFeePerGas" : "0x0a",
            "bloom" : "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            "coinbase" : "0x2adc25665018aa1fe0e6bc666dac8fc2697ff9ba",
            "difficulty" : "0x020000",
            "extraData" : "0x00",
            "gasLimit" : "0x10000000000000",
            "gasUsed" : "0x10000000000000",
            "hash" : "0x7ebfee2a2c785fef181b8ffd92d4a48a0660ec000f465f309757e3f092d13882",
            "mixHash" : "0x0000000000000000000000000000000000000000000000000000000000000000",
            "nonce" : "0x0000000000000000",
            "number" : "0x01",
            "parentHash" : "0xa8f2eb2ea9dccbf725801eef5a31ce59bada431e888dfd5501677cc4365dc3be",
            "receiptTrie" : "0xbdd943f5c62ae0299324244a0f65524337ada9817e18e1764631cc1424f3a293",
            "stateRoot" : "0xc9c6306ee3e5acbaabe8e2fa28a10c12e27bad1d1aacc271665149f70519f8b0",
            "timestamp" : "0x03e8",
            "transactionsTrie" : "0xf5893b055ca05e4f14d1792745586a1376e218180bd56bd96b2b024e1dc78300",
            "uncleHash" : "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"
        }"#;
        let res = serde_json::from_str::<Header>(test);
        assert!(res.is_ok(), "Failed to deserialize Header with error: {res:?}");
    }

    #[test]
    fn transaction_deserialize() {
        let test = r#"[
            {
                "accessList" : [
                ],
                "chainId" : "0x01",
                "data" : "0x693c61390000000000000000000000000000000000000000000000000000000000000000",
                "gasLimit" : "0x10000000000000",
                "maxFeePerGas" : "0x07d0",
                "maxPriorityFeePerGas" : "0x00",
                "nonce" : "0x01",
                "r" : "0x5fecc3972a35c9e341b41b0c269d9a7325e13269fb01c2f64cbce1046b3441c8",
                "s" : "0x7d4d0eda0e4ebd53c5d0b6fc35c600b317f8fa873b3963ab623ec9cec7d969bd",
                "sender" : "0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b",
                "to" : "0xcccccccccccccccccccccccccccccccccccccccc",
                "type" : "0x02",
                "v" : "0x01",
                "value" : "0x00"
            }
        ]"#;

        let res = serde_json::from_str::<Vec<Transaction>>(test);
        assert!(res.is_ok(), "Failed to deserialize transaction with error: {res:?}");
    }
}
