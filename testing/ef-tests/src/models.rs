//! Shared models for <https://github.com/ethereum/tests>

use crate::{assert::assert_equal, Error};
use reth_db::{
    cursor::DbDupCursorRO,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{
    keccak256, Account as RethAccount, Address, BigEndianHash, Bloom, Bytecode, Bytes, ChainSpec,
    ChainSpecBuilder, Header as RethHeader, JsonU256, SealedHeader, StorageEntry, Withdrawal, H160,
    H256, H64, U256,
};
use serde::{self, Deserialize};
use std::{collections::BTreeMap, ops::Deref};

/// The definition of a blockchain test.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockchainTest {
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
    #[serde(rename = "_info")]
    #[allow(unused)]
    info: BlockchainTestInfo,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct BlockchainTestInfo {
    #[serde(rename = "filling-rpc-server")]
    #[allow(unused)]
    // One test has an invalid string in this field, which breaks our CI:
    // https://github.com/ethereum/tests/blob/6c252923bdd1bd5a70f680df1214f866f76839db/GeneralStateTests/stTransactionTest/ValueOverflow.json#L5
    // By using `serde_bytes::ByteBuf`, we ignore the validation of this field as a string.
    // TODO(alexey): remove when `ethereum/tests` is fixed
    filling_rpc_server: serde_bytes::ByteBuf,
}

/// A block header in an Ethereum blockchain test.
#[derive(Debug, PartialEq, Eq, Clone, Deserialize)]
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
    /// Withdrawals root.
    pub withdrawals_root: Option<H256>,
}

impl From<Header> for SealedHeader {
    fn from(value: Header) -> Self {
        let header = RethHeader {
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
            logs_bloom: value.bloom,
            withdrawals_root: value.withdrawals_root,
        };
        header.seal(value.hash)
    }
}

/// A block in an Ethereum blockchain test.
#[derive(Debug, PartialEq, Eq, Deserialize)]
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

/// Transaction sequence in block
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct TransactionSequence {
    exception: String,
    raw_bytes: Bytes,
    valid: String,
}

/// Ethereum blockchain test data state.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct State(BTreeMap<Address, Account>);

impl State {
    /// Write the state to the database.
    pub fn write_to_db<'a, Tx>(&self, tx: &'a Tx) -> Result<(), Error>
    where
        Tx: DbTxMut<'a>,
    {
        for (&address, account) in self.0.iter() {
            let has_code = !account.code.is_empty();
            let code_hash = has_code.then(|| keccak256(&account.code));
            tx.put::<tables::PlainAccountState>(
                address,
                RethAccount {
                    balance: account.balance.0,
                    nonce: account.nonce.0.to::<u64>(),
                    bytecode_hash: code_hash,
                },
            )?;
            if let Some(code_hash) = code_hash {
                tx.put::<tables::Bytecodes>(code_hash, Bytecode::new_raw(account.code.0.clone()))?;
            }
            account.storage.iter().try_for_each(|(k, v)| {
                tx.put::<tables::PlainStorageState>(
                    address,
                    StorageEntry { key: H256::from_slice(&k.0.to_be_bytes::<32>()), value: v.0 },
                )
            })?;
        }

        Ok(())
    }
}

impl Deref for State {
    type Target = BTreeMap<Address, Account>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

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

impl Account {
    /// Check that the account matches what is in the database.
    ///
    /// In case of a mismatch, `Err(Error::Assertion)` is returned.
    pub fn assert_db<'a, Tx>(&self, address: Address, tx: &'a Tx) -> Result<(), Error>
    where
        Tx: DbTx<'a>,
    {
        let account = tx.get::<tables::PlainAccountState>(address)?.ok_or_else(|| {
            Error::Assertion(format!("Account is missing ({address}) expected: {:?}", self))
        })?;

        assert_equal(self.balance.into(), account.balance, "Balance does not match")?;
        assert_equal(self.nonce.0.to(), account.nonce, "Nonce does not match")?;

        if let Some(bytecode_hash) = account.bytecode_hash {
            assert_equal(keccak256(&self.code), bytecode_hash, "Bytecode does not match")?;
        } else {
            assert_equal(
                self.code.is_empty(),
                true,
                "Expected empty bytecode, got bytecode in db.",
            )?;
        }

        let mut storage_cursor = tx.cursor_dup_read::<tables::PlainStorageState>()?;
        for (slot, value) in self.storage.iter() {
            if let Some(entry) =
                storage_cursor.seek_by_key_subkey(address, H256(slot.0.to_be_bytes()))?
            {
                if U256::from_be_bytes(entry.key.0) == slot.0 {
                    assert_equal(
                        value.0,
                        entry.value,
                        &format!("Storage for slot {:?} does not match", slot),
                    )?;
                } else {
                    return Err(Error::Assertion(format!(
                        "Slot {:?} is missing from the database. Expected {:?}",
                        slot, value
                    )))
                }
            } else {
                return Err(Error::Assertion(format!(
                    "Slot {:?} is missing from the database. Expected {:?}",
                    slot, value
                )))
            }
        }

        Ok(())
    }
}

/// Fork specification.
#[derive(Debug, PartialEq, Eq, PartialOrd, Hash, Ord, Clone, Deserialize)]
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
            ForkSpec::Shanghai => spec_builder.shanghai_activated(),
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
