//! Shared models for <https://github.com/ethereum/tests>

use crate::{assert::assert_equal, Error};
use alloy_consensus::Header as RethHeader;
use alloy_eips::{eip4844::DATA_GAS_PER_BLOB, eip4895::Withdrawals, eip7840::BlobParams};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{
    keccak256, map::HashMap, Address, Bloom, Bytes, B256, B64, U128, U256, U64,
};
use reth_chainspec::{Chain, ChainSpec, ChainSpecBuilder, EthereumHardfork, ForkCondition};
use reth_db_api::{cursor::DbDupCursorRO, tables, transaction::DbTx};
use reth_primitives_traits::SealedHeader;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    ops::Deref,
    sync::{Arc, OnceLock, RwLock},
};

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
    pub post_state: Option<BTreeMap<Address, Account>>,
    /// The test pre-state.
    pub pre: State,
    /// Hash of the best block.
    pub lastblockhash: B256,
    /// Network spec.
    pub network: ForkSpec,
    /// Chain ID and blob schedule supplied by the fixture.
    #[serde(default)]
    pub config: FixtureConfig,
    /// Engine spec.
    #[serde(default)]
    pub seal_engine: SealEngine,
}

impl BlockchainTest {
    /// Builds the fixture's fork schedule, chain ID, blob parameters and genesis allocation.
    ///
    /// # Errors
    /// Returns an error for unsupported forks, out-of-range genesis quantities, or invalid blob
    /// parameters.
    pub fn chain_spec(&self) -> Result<ChainSpec, FixtureConfigError> {
        fixture_chain_spec(&self.genesis_block_header, &self.pre, self.network, &self.config)
    }
}

/// A blockchain test in the engine format, whose blocks are Engine API payloads.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineTest {
    /// Genesis block header.
    pub genesis_block_header: Header,
    /// The test pre-state.
    pub pre: State,
    /// Hash of the best block.
    pub lastblockhash: B256,
    /// Network spec.
    pub network: ForkSpec,
    /// Chain ID and blob schedule supplied by the fixture.
    #[serde(default)]
    pub config: FixtureConfig,
    /// The payloads to send, in order.
    pub engine_new_payloads: Vec<EngineNewPayload>,
}

impl EngineTest {
    /// Returns the chain spec of the test's network, with the test's genesis block.
    ///
    /// # Errors
    /// Returns an error for unsupported forks, out-of-range genesis quantities, or invalid blob
    /// parameters.
    pub fn chain_spec(&self) -> Result<ChainSpec, FixtureConfigError> {
        fixture_chain_spec(&self.genesis_block_header, &self.pre, self.network, &self.config)
    }
}

/// One `engine_newPayload` call of an [`EngineTest`].
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineNewPayload {
    /// The call's parameters, sent to the client as they are.
    pub params: Vec<serde_json::Value>,
    /// The version of `engine_newPayload` to call.
    pub new_payload_version: String,
    /// The version of `engine_forkchoiceUpdated` to call once the payload is accepted.
    pub forkchoice_updated_version: String,
    /// The expected validation error, if the payload is invalid.
    pub validation_error: Option<String>,
    /// The expected JSON-RPC error code, if the call must fail.
    pub error_code: Option<String>,
}

/// Execution configuration embedded in a fixture.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureConfig {
    /// The chain ID; absent legacy configurations use mainnet's ID.
    #[serde(rename = "chainid")]
    pub chain_id: Option<U64>,
    /// Per-fork blob parameters. Unlisted forks retain their protocol defaults.
    #[serde(default)]
    pub blob_schedule: BTreeMap<ForkSpec, FixtureBlobParams>,
}

/// Quantity-encoded EIP-7840 parameters from the fixture format.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureBlobParams {
    /// Target number of blobs in a block.
    pub target: U64,
    /// Maximum number of blobs in a block.
    pub max: U64,
    /// Nonzero denominator used to update the blob base fee.
    #[serde(deserialize_with = "deserialize_blob_update_fraction")]
    pub base_fee_update_fraction: U128,
}

/// A fixture configuration that the runner cannot execute.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FixtureConfigError {
    /// An experimental fork has no matching EVM configuration.
    #[error("unsupported fixture fork: {0:?}")]
    UnsupportedFork(ForkSpec),
    /// A genesis header quantity exceeds the client's representation.
    #[error("fixture genesis {field} does not fit in u64: {source}")]
    HeaderQuantity {
        /// The fixture's header field.
        field: &'static str,
        /// The failed checked conversion.
        #[source]
        source: alloy_primitives::ruint::FromUintError<u64>,
    },
    /// Blob parameters would overflow or divide by zero during header validation.
    #[error("unsupported fixture blob schedule for {fork:?}: target {target}, maximum {max}")]
    BlobParams {
        /// The fork whose parameters cannot be executed.
        fork: ForkSpec,
        /// The configured target.
        target: u64,
        /// The configured maximum.
        max: u64,
    },
}

/// A block header in an Ethereum blockchain test.
#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    /// Bloom filter.
    pub bloom: Bloom,
    /// Coinbase.
    pub coinbase: Address,
    /// Difficulty.
    pub difficulty: U256,
    /// Extra data.
    pub extra_data: Bytes,
    /// Gas limit.
    pub gas_limit: U256,
    /// Gas used.
    pub gas_used: U256,
    /// Block Hash.
    pub hash: B256,
    /// Mix hash.
    pub mix_hash: B256,
    /// Seal nonce.
    pub nonce: B64,
    /// Block number.
    pub number: U256,
    /// Parent hash.
    pub parent_hash: B256,
    /// Receipt trie.
    pub receipt_trie: B256,
    /// State root.
    pub state_root: B256,
    /// Timestamp.
    pub timestamp: U256,
    /// Transactions trie.
    pub transactions_trie: B256,
    /// Uncle hash.
    pub uncle_hash: B256,
    /// Base fee per gas.
    pub base_fee_per_gas: Option<U256>,
    /// Withdrawals root.
    pub withdrawals_root: Option<B256>,
    /// Blob gas used.
    pub blob_gas_used: Option<U256>,
    /// Excess blob gas.
    pub excess_blob_gas: Option<U256>,
    /// Parent beacon block root.
    pub parent_beacon_block_root: Option<B256>,
    /// Requests root.
    pub requests_hash: Option<B256>,
    /// Target blobs per block.
    pub target_blobs_per_block: Option<U256>,
    /// Block access list hash.
    pub block_access_list_hash: Option<B256>,
    /// Slot number.
    pub slot_number: Option<U256>,
}

impl TryFrom<Header> for SealedHeader {
    type Error = FixtureConfigError;

    fn try_from(value: Header) -> Result<Self, Self::Error> {
        let quantity = |value: U256, field| {
            u64::try_from(value)
                .map_err(|source| FixtureConfigError::HeaderQuantity { field, source })
        };
        let header = RethHeader {
            base_fee_per_gas: value
                .base_fee_per_gas
                .map(|v| quantity(v, "baseFeePerGas"))
                .transpose()?,
            beneficiary: value.coinbase,
            difficulty: value.difficulty,
            extra_data: value.extra_data,
            gas_limit: quantity(value.gas_limit, "gasLimit")?,
            gas_used: quantity(value.gas_used, "gasUsed")?,
            mix_hash: value.mix_hash,
            nonce: u64::from_be_bytes(value.nonce.0).into(),
            number: quantity(value.number, "number")?,
            timestamp: quantity(value.timestamp, "timestamp")?,
            transactions_root: value.transactions_trie,
            receipts_root: value.receipt_trie,
            ommers_hash: value.uncle_hash,
            state_root: value.state_root,
            parent_hash: value.parent_hash,
            logs_bloom: value.bloom,
            withdrawals_root: value.withdrawals_root,
            blob_gas_used: value.blob_gas_used.map(|v| quantity(v, "blobGasUsed")).transpose()?,
            excess_blob_gas: value
                .excess_blob_gas
                .map(|v| quantity(v, "excessBlobGas"))
                .transpose()?,
            parent_beacon_block_root: value.parent_beacon_block_root,
            requests_hash: value.requests_hash,
            block_access_list_hash: value.block_access_list_hash,
            slot_number: value.slot_number.map(|v| quantity(v, "slotNumber")).transpose()?,
        };
        Ok(Self::new(header, value.hash))
    }
}

/// A block in an Ethereum blockchain test.
#[derive(Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    /// Block header.
    pub block_header: Option<Header>,
    /// RLP encoded block bytes
    pub rlp: Bytes,
    /// If the execution of the block should fail,
    /// `expect_exception` is `Some`.
    /// Its contents detail the reason for the failure.
    pub expect_exception: Option<String>,
    /// Transactions
    pub transactions: Option<Vec<Transaction>>,
    /// Uncle/ommer headers
    pub uncle_headers: Option<Vec<Header>>,
    /// Transaction Sequence
    pub transaction_sequence: Option<Vec<TransactionSequence>>,
    /// Withdrawals
    pub withdrawals: Option<Withdrawals>,
    /// The block's access list (EIP-7928), delivered beside the block. Kept as JSON so that a
    /// malformed list is dropped instead of failing the whole file.
    pub block_access_list: Option<serde_json::Value>,
    /// The decoded fields of a block that is expected to be invalid.
    #[serde(rename = "rlp_decoded")]
    pub rlp_decoded: Option<RlpDecodedBlock>,
}

impl Block {
    /// Returns the access list delivered with this block, if any.
    pub fn access_list(&self) -> Option<&serde_json::Value> {
        self.block_access_list
            .as_ref()
            .or_else(|| self.rlp_decoded.as_ref()?.block_access_list.as_ref())
    }
}

/// The decoded fields of a block that is expected to be invalid.
#[derive(Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RlpDecodedBlock {
    /// The block's access list (EIP-7928).
    pub block_access_list: Option<serde_json::Value>,
}

/// Transaction sequence in block
#[derive(Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct TransactionSequence {
    exception: String,
    raw_bytes: Bytes,
    valid: String,
}

/// Ethereum blockchain test data state.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Default)]
pub struct State(BTreeMap<Address, Account>);

impl State {
    /// Return state as genesis state.
    pub fn into_genesis_state(self) -> BTreeMap<Address, GenesisAccount> {
        self.0
            .into_iter()
            .map(|(address, account)| {
                let storage = account
                    .storage
                    .iter()
                    .filter(|(_, v)| !v.is_zero())
                    .map(|(k, v)| (B256::from(*k), B256::from(*v)))
                    .collect();
                let account = GenesisAccount {
                    balance: account.balance,
                    nonce: Some(account.nonce.try_into().unwrap()),
                    code: Some(account.code).filter(|c| !c.is_empty()),
                    storage: Some(storage),
                    ..Default::default()
                };
                (address, account)
            })
            .collect::<BTreeMap<_, _>>()
    }
}

impl Deref for State {
    type Target = BTreeMap<Address, Account>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// An account.
#[derive(Debug, PartialEq, Eq, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Account {
    /// Balance.
    pub balance: U256,
    /// Code.
    pub code: Bytes,
    /// Nonce.
    pub nonce: U256,
    /// Storage.
    pub storage: BTreeMap<U256, U256>,
}

impl Account {
    /// Check that the account matches what is in the database.
    ///
    /// In case of a mismatch, `Err(Error::Assertion)` is returned.
    pub fn assert_db(&self, address: Address, tx: &impl DbTx) -> Result<(), Error> {
        let account =
            tx.get_by_encoded_key::<tables::PlainAccountState>(&address)?.ok_or_else(|| {
                Error::Assertion(format!(
                    "Expected account ({address}) is missing from DB: {self:?}"
                ))
            })?;

        assert_equal(self.balance, account.balance, "Balance does not match")?;
        assert_equal(self.nonce.to(), account.nonce, "Nonce does not match")?;

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
        for (slot, value) in &self.storage {
            if let Some(entry) = storage_cursor.seek_by_key_subkey(address, B256::from(*slot))? {
                if U256::from_be_bytes(entry.key.0) == *slot {
                    assert_equal(
                        *value,
                        entry.value,
                        &format!("Storage for slot {slot:?} does not match"),
                    )?;
                } else {
                    return Err(Error::Assertion(format!(
                        "Slot {slot:?} is missing from the database. Expected {value:?}"
                    )))
                }
            } else {
                return Err(Error::Assertion(format!(
                    "Slot {slot:?} is missing from the database. Expected {value:?}"
                )))
            }
        }

        Ok(())
    }
}

/// Fork specification.
#[derive(Debug, PartialEq, Eq, PartialOrd, Hash, Ord, Clone, Copy, Deserialize)]
pub enum ForkSpec {
    /// Frontier
    Frontier,
    /// Frontier to Homestead
    FrontierToHomesteadAt5,
    /// Homestead
    Homestead,
    /// Homestead to DAO
    HomesteadToDaoAt5,
    /// Homestead to Tangerine
    HomesteadToEIP150At5,
    /// Tangerine
    #[serde(alias = "TangerineWhistle")]
    EIP150,
    /// Spurious Dragon
    #[serde(alias = "SpuriousDragon")]
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
    #[serde(alias = "Paris")]
    Merge,
    /// Paris to Shanghai at time 15k
    ParisToShanghaiAtTime15k,
    /// Shanghai
    Shanghai,
    /// Shanghai to Cancun at time 15k
    ShanghaiToCancunAtTime15k,
    /// Merge EOF test
    #[serde(alias = "Merge+3540+3670")]
    MergeEOF,
    /// After Merge Init Code test
    #[serde(alias = "Merge+3860")]
    MergeMeterInitCode,
    /// After Merge plus new PUSH0 opcode
    #[serde(alias = "Merge+3855")]
    MergePush0,
    /// Cancun
    Cancun,
    /// Cancun to Prague at time 15k
    CancunToPragueAtTime15k,
    /// Prague
    Prague,
    /// Osaka
    Osaka,
    /// Amsterdam.
    Amsterdam,
    /// Prague to Osaka at timestamp 15,000.
    PragueToOsakaAtTime15k,
    /// First mainnet blob-parameter-only update.
    BPO1,
    /// Second mainnet blob-parameter-only update.
    BPO2,
    /// Osaka to BPO1 at timestamp 15,000.
    OsakaToBPO1AtTime15k,
    /// BPO1 to BPO2 at timestamp 15,000.
    BPO1ToBPO2AtTime15k,
    /// BPO2 to Amsterdam at timestamp 15,000.
    BPO2ToAmsterdamAtTime15k,
    /// Amsterdam with the fixture's increased blob parameters.
    AmsterdamBPOIncrease,
    /// Amsterdam with the fixture's decreased blob parameters.
    AmsterdamBPODecrease,
    /// Amsterdam to increased blob parameters at timestamp 15,000.
    AmsterdamToBPOIncreaseAtTime15k,
    /// Amsterdam to decreased blob parameters at timestamp 15,000.
    AmsterdamToBPODecreaseAtTime15k,
}

impl ForkSpec {
    /// Converts this EF fork spec to a Reth [`ChainSpec`].
    pub fn to_chain_spec(self) -> Arc<ChainSpec> {
        static MAP: OnceLock<RwLock<HashMap<ForkSpec, Arc<ChainSpec>>>> = OnceLock::new();
        let map = MAP.get_or_init(Default::default);
        if let Some(r) = map.read().unwrap().get(&self) {
            return r.clone();
        }
        map.write()
            .unwrap()
            .entry(self)
            .or_insert_with(|| Arc::new(self.to_chain_spec_inner()))
            .clone()
    }

    fn to_chain_spec_inner(self) -> ChainSpec {
        self.build_chain_spec(self.chain_spec_builder())
    }

    /// Returns the chain spec of this fork spec on the given genesis.
    pub fn chain_spec_on(self, genesis: Genesis) -> ChainSpec {
        self.build_chain_spec(self.chain_spec_builder().genesis(genesis))
    }

    fn build_chain_spec(self, builder: ChainSpecBuilder) -> ChainSpec {
        let mut spec = builder.build();

        let scheduled = match self {
            Self::BPO1 => vec![(0, BlobParams::bpo1())],
            Self::BPO2 | Self::Amsterdam | Self::BPO2ToAmsterdamAtTime15k => {
                vec![(0, BlobParams::bpo2())]
            }
            Self::OsakaToBPO1AtTime15k => vec![(15_000, BlobParams::bpo1())],
            Self::BPO1ToBPO2AtTime15k => {
                vec![(0, BlobParams::bpo1()), (15_000, BlobParams::bpo2())]
            }
            Self::AmsterdamBPOIncrease |
            Self::AmsterdamBPODecrease |
            Self::AmsterdamToBPOIncreaseAtTime15k |
            Self::AmsterdamToBPODecreaseAtTime15k => vec![(0, BlobParams::bpo2())],
            _ => Vec::new(),
        };
        spec.blob_params = spec.blob_params.with_scheduled(scheduled);
        spec
    }

    /// Returns a builder of the chain spec with the hardforks of this fork spec, on the mainnet
    /// genesis.
    fn chain_spec_builder(self) -> ChainSpecBuilder {
        let spec_builder = ChainSpecBuilder::mainnet().reset();

        match self {
            Self::Frontier => spec_builder.frontier_activated(),
            Self::FrontierToHomesteadAt5 => spec_builder
                .frontier_activated()
                .with_fork(EthereumHardfork::Homestead, ForkCondition::Block(5)),
            Self::Homestead => spec_builder.homestead_activated(),
            Self::HomesteadToDaoAt5 => spec_builder
                .homestead_activated()
                .with_fork(EthereumHardfork::Dao, ForkCondition::Block(5)),
            Self::HomesteadToEIP150At5 => spec_builder
                .homestead_activated()
                .with_fork(EthereumHardfork::Tangerine, ForkCondition::Block(5)),
            Self::EIP150 => spec_builder.tangerine_whistle_activated(),
            Self::EIP158 => spec_builder.spurious_dragon_activated(),
            Self::EIP158ToByzantiumAt5 => spec_builder
                .spurious_dragon_activated()
                .with_fork(EthereumHardfork::Byzantium, ForkCondition::Block(5)),
            Self::Byzantium => spec_builder.byzantium_activated(),
            Self::ByzantiumToConstantinopleAt5 => spec_builder
                .byzantium_activated()
                .with_fork(EthereumHardfork::Constantinople, ForkCondition::Block(5)),
            Self::ByzantiumToConstantinopleFixAt5 => spec_builder
                .byzantium_activated()
                .with_fork(EthereumHardfork::Petersburg, ForkCondition::Block(5)),
            Self::Constantinople => spec_builder.constantinople_activated(),
            Self::ConstantinopleFix => spec_builder.petersburg_activated(),
            Self::Istanbul => spec_builder.istanbul_activated(),
            Self::Berlin => spec_builder.berlin_activated(),
            Self::BerlinToLondonAt5 => spec_builder
                .berlin_activated()
                .with_fork(EthereumHardfork::London, ForkCondition::Block(5)),
            Self::London => spec_builder.london_activated(),
            Self::Merge | Self::MergeEOF | Self::MergeMeterInitCode | Self::MergePush0 => {
                spec_builder.paris_activated()
            }
            Self::ParisToShanghaiAtTime15k => spec_builder
                .paris_activated()
                .with_fork(EthereumHardfork::Shanghai, ForkCondition::Timestamp(15_000)),
            Self::Shanghai => spec_builder.shanghai_activated(),
            Self::ShanghaiToCancunAtTime15k => spec_builder
                .shanghai_activated()
                .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(15_000)),
            Self::Cancun => spec_builder.cancun_activated(),
            Self::CancunToPragueAtTime15k => spec_builder
                .cancun_activated()
                .with_fork(EthereumHardfork::Prague, ForkCondition::Timestamp(15_000)),
            Self::Prague => spec_builder.prague_activated(),
            Self::Osaka => spec_builder.osaka_activated(),
            Self::Amsterdam => spec_builder.amsterdam_activated(),
            Self::PragueToOsakaAtTime15k => spec_builder
                .prague_activated()
                .with_fork(EthereumHardfork::Osaka, ForkCondition::Timestamp(15_000)),
            Self::BPO1 | Self::OsakaToBPO1AtTime15k => spec_builder.osaka_activated().with_fork(
                EthereumHardfork::Bpo1,
                ForkCondition::Timestamp(if self == Self::BPO1 { 0 } else { 15_000 }),
            ),
            Self::BPO2 | Self::BPO1ToBPO2AtTime15k | Self::BPO2ToAmsterdamAtTime15k => {
                let builder = spec_builder
                    .osaka_activated()
                    .with_fork(EthereumHardfork::Bpo1, ForkCondition::Timestamp(0))
                    .with_fork(
                        EthereumHardfork::Bpo2,
                        ForkCondition::Timestamp(if self == Self::BPO1ToBPO2AtTime15k {
                            15_000
                        } else {
                            0
                        }),
                    );
                if self == Self::BPO2ToAmsterdamAtTime15k {
                    builder.with_fork(EthereumHardfork::Amsterdam, ForkCondition::Timestamp(15_000))
                } else {
                    builder
                }
            }
            Self::AmsterdamBPOIncrease |
            Self::AmsterdamBPODecrease |
            Self::AmsterdamToBPOIncreaseAtTime15k |
            Self::AmsterdamToBPODecreaseAtTime15k => {
                let timestamp =
                    if matches!(self, Self::AmsterdamBPOIncrease | Self::AmsterdamBPODecrease) {
                        0
                    } else {
                        15_000
                    };
                spec_builder
                    .amsterdam_activated()
                    .with_fork(EthereumHardfork::Bpo1, ForkCondition::Timestamp(0))
                    .with_fork(EthereumHardfork::Bpo2, ForkCondition::Timestamp(0))
                    .with_fork(EthereumHardfork::Bpo3, ForkCondition::Timestamp(timestamp))
            }
        }
    }
}

/// Possible seal engines.
#[derive(Debug, PartialEq, Eq, Default, Deserialize)]
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
    pub transaction_type: Option<U256>,
    /// Data.
    pub data: Bytes,
    /// Gas limit.
    pub gas_limit: U256,
    /// Gas price.
    pub gas_price: Option<U256>,
    /// Nonce.
    pub nonce: U256,
    /// Signature r part.
    pub r: U256,
    /// Signature s part.
    pub s: U256,
    /// Parity bit.
    pub v: U256,
    /// Transaction value.
    pub value: U256,
    /// Chain ID.
    pub chain_id: Option<U256>,
    /// Access list.
    pub access_list: Option<AccessList>,
    /// Max fee per gas.
    pub max_fee_per_gas: Option<U256>,
    /// Max priority fee per gas
    pub max_priority_fee_per_gas: Option<U256>,
    /// Transaction hash.
    pub hash: Option<B256>,
}

/// Access list item
#[derive(Debug, PartialEq, Eq, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccessListItem {
    /// Account address
    pub address: Address,
    /// Storage key.
    pub storage_keys: Vec<B256>,
}

/// Access list.
pub type AccessList = Vec<AccessListItem>;

// Fixture quantities need their own decoder because BlobParams' serde form uses JSON numbers.
fn deserialize_blob_update_fraction<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<U128, D::Error> {
    let fraction = U128::deserialize(deserializer)?;
    if fraction.is_zero() {
        return Err(serde::de::Error::custom("blob base fee update fraction must be nonzero"))
    }
    Ok(fraction)
}

fn fixture_chain_spec(
    header: &Header,
    pre: &State,
    network: ForkSpec,
    config: &FixtureConfig,
) -> Result<ChainSpec, FixtureConfigError> {
    if matches!(network, ForkSpec::MergeEOF | ForkSpec::MergeMeterInitCode | ForkSpec::MergePush0) {
        return Err(FixtureConfigError::UnsupportedFork(network))
    }
    let header = SealedHeader::try_from(header.clone())?;
    let genesis = Genesis::default()
        .with_nonce(u64::from_be_bytes(header.nonce.0))
        .with_timestamp(header.timestamp)
        .with_extra_data(header.extra_data.clone())
        .with_gas_limit(header.gas_limit)
        .with_difficulty(header.difficulty)
        .with_mix_hash(header.mix_hash)
        .with_coinbase(header.beneficiary)
        .with_base_fee(header.base_fee_per_gas.map(u128::from))
        .with_excess_blob_gas(header.excess_blob_gas)
        .with_blob_gas_used(header.blob_gas_used)
        .with_slot_number(header.slot_number)
        .extend_accounts(pre.clone().into_genesis_state());
    let mut spec = network.chain_spec_on(genesis);
    if let Some(chain_id) = config.chain_id {
        spec.chain = Chain::from_id(chain_id.to());
    }
    spec.blob_params.cancun =
        fixture_blob_params(ForkSpec::Cancun, spec.blob_params.cancun, config)?;
    spec.blob_params.prague =
        fixture_blob_params(ForkSpec::Prague, spec.blob_params.prague, config)?;
    spec.blob_params.osaka = fixture_blob_params(ForkSpec::Osaka, spec.blob_params.osaka, config)?;

    let bpo3 = match network {
        ForkSpec::AmsterdamBPOIncrease | ForkSpec::AmsterdamToBPOIncreaseAtTime15k => {
            Some(ForkSpec::AmsterdamBPOIncrease)
        }
        ForkSpec::AmsterdamBPODecrease | ForkSpec::AmsterdamToBPODecreaseAtTime15k => {
            Some(ForkSpec::AmsterdamBPODecrease)
        }
        _ => None,
    };
    // Apply forks in protocol order, including defaults. Later forks win when genesis activates
    // multiple forks at the same timestamp, independently of ForkSpec's declaration order.
    let forks = [
        (ForkSpec::BPO1, EthereumHardfork::Bpo1, BlobParams::bpo1()),
        (ForkSpec::BPO2, EthereumHardfork::Bpo2, BlobParams::bpo2()),
        (ForkSpec::Amsterdam, EthereumHardfork::Amsterdam, BlobParams::bpo2()),
    ];
    let mut scheduled = BTreeMap::new();
    for (fork, hardfork, defaults) in
        forks.into_iter().chain(bpo3.map(|fork| (fork, EthereumHardfork::Bpo3, BlobParams::bpo2())))
    {
        if let Some(timestamp) = spec.hardforks.fork(hardfork).as_timestamp() {
            scheduled.insert(timestamp, fixture_blob_params(fork, defaults, config)?);
        }
    }
    spec.blob_params.scheduled = scheduled.into_iter().collect();
    Ok(spec)
}

fn fixture_blob_params(
    fork: ForkSpec,
    mut params: BlobParams,
    config: &FixtureConfig,
) -> Result<BlobParams, FixtureConfigError> {
    let Some(fixture) = config.blob_schedule.get(&fork) else { return Ok(params) };
    params.target_blob_count = fixture.target.to();
    params.max_blob_count = fixture.max.to();
    params.update_fraction = fixture.base_fee_update_fraction.to();
    if params.target_blob_count > params.max_blob_count ||
        params.max_blob_count > u64::MAX / DATA_GAS_PER_BLOB ||
        (params.blob_base_cost != 0 && params.max_blob_count == 0)
    {
        return Err(FixtureConfigError::BlobParams {
            fork,
            target: params.target_blob_count,
            max: params.max_blob_count,
        })
    }
    if matches!(fork, ForkSpec::Cancun | ForkSpec::Prague) {
        params.max_blobs_per_tx = params.max_blob_count;
    }
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxEip4844};
    use alloy_eips::eip4844::VERSIONED_HASH_VERSION_KZG;
    use alloy_primitives::Signature;
    use reth_chainspec::EthChainSpec;
    use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
    use reth_evm::{database::StateProviderDatabase, ConfigureEvm, Evm, EvmEnv};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::SignedTransaction;
    use reth_provider::{
        test_utils::{ExtendedAccount, MockEthProvider},
        StateProvider,
    };

    #[test]
    fn amsterdam_uses_bpo2_blob_params_on_any_genesis() {
        for spec in [
            ForkSpec::Amsterdam.to_chain_spec(),
            Arc::new(ForkSpec::Amsterdam.chain_spec_on(Genesis::default())),
        ] {
            assert_eq!(spec.blob_params_at_timestamp(0), Some(BlobParams::bpo2()));
        }
    }

    #[test]
    fn fixture_chain_id_and_blob_schedule_are_applied() {
        let config: FixtureConfig = serde_json::from_value(serde_json::json!({
            "chainid": "0x07",
            "blobSchedule": {
                "Amsterdam": {"target": "0x0e", "max": "0x15", "baseFeeUpdateFraction": "0xb24b3f"},
                "AmsterdamBPOIncrease": {"target": "0x15", "max": "0x20", "baseFeeUpdateFraction": "0x10b70de"}
            }
        })).unwrap();
        let spec = fixture_chain_spec(
            &Header::default(),
            &State::default(),
            ForkSpec::AmsterdamToBPOIncreaseAtTime15k,
            &config,
        )
        .unwrap();
        assert_eq!(spec.chain.id(), 7);
        assert_eq!(spec.blob_params_at_timestamp(14_999).unwrap().target_blob_count, 14);
        assert_eq!(spec.blob_params_at_timestamp(15_000).unwrap().target_blob_count, 21);
        assert_eq!(spec.blob_params_at_timestamp(15_000).unwrap().max_blob_count, 32);
    }

    #[test]
    fn bpo_transition_uses_the_fixture_activation_time() {
        let spec = ForkSpec::BPO1ToBPO2AtTime15k.chain_spec_on(Genesis::default());
        assert_eq!(spec.blob_params_at_timestamp(14_999), Some(BlobParams::bpo1()));
        assert_eq!(spec.blob_params_at_timestamp(15_000), Some(BlobParams::bpo2()));
    }

    #[test]
    fn fixture_aliases_and_zero_blob_fraction() {
        assert_eq!(
            serde_json::from_str::<ForkSpec>("\"SpuriousDragon\"").unwrap(),
            ForkSpec::EIP158
        );
        assert_eq!(
            serde_json::from_str::<ForkSpec>("\"TangerineWhistle\"").unwrap(),
            ForkSpec::EIP150
        );
        let error = serde_json::from_value::<FixtureConfig>(serde_json::json!({
            "blobSchedule": {"BPO1": {"target": "0xa", "max": "0xf", "baseFeeUpdateFraction": "0x0"}}
        })).unwrap_err();
        assert_eq!(error.to_string(), "blob base fee update fraction must be nonzero");
    }

    #[test]
    fn fixture_blob_maximum_applies_during_transaction_execution() {
        for (fork, max, count) in [(ForkSpec::Cancun, 9, 7), (ForkSpec::Prague, 12, 10)] {
            let config = FixtureConfig {
                blob_schedule: BTreeMap::from([(
                    fork,
                    FixtureBlobParams {
                        target: U64::from(3),
                        max: U64::from(max),
                        base_fee_update_fraction: U128::from(3_338_477),
                    },
                )]),
                ..Default::default()
            };
            let spec =
                fixture_chain_spec(&Header::default(), &State::default(), fork, &config).unwrap();
            let evm_config = EthEvmConfig::new(Arc::new(spec));
            let env = evm_config
                .evm_env(&RethHeader {
                    gas_limit: 30_000_000,
                    base_fee_per_gas: Some(0),
                    excess_blob_gas: Some(0),
                    ..Default::default()
                })
                .unwrap();
            let sender = Address::with_last_byte(0x42);
            let provider = MockEthProvider::<EthPrimitives>::default();
            provider.add_account(sender, ExtendedAccount::new(0, U256::MAX));
            let tx = TransactionSigned::Eip4844(
                TxEip4844 {
                    chain_id: 1,
                    gas_limit: 21_000,
                    max_fee_per_blob_gas: 1,
                    blob_versioned_hashes: vec![
                        B256::repeat_byte(VERSIONED_HASH_VERSION_KZG);
                        count
                    ],
                    ..Default::default()
                }
                .into_signed(Signature::test_signature()),
            );
            let tx = evm_config.tx_env(tx.with_signer(sender));
            let mut evm = evm_config
                .evm_with_env(StateProviderDatabase(provider.into_evm_state_provider()), env);
            evm.transact_and_discard(&tx, |_| {}).unwrap();
        }
        let config = FixtureConfig {
            blob_schedule: BTreeMap::from([(
                ForkSpec::Osaka,
                FixtureBlobParams {
                    target: U64::from(6),
                    max: U64::from(12),
                    base_fee_update_fraction: U128::from(5_007_716),
                },
            )]),
            ..Default::default()
        };
        let spec =
            fixture_chain_spec(&Header::default(), &State::default(), ForkSpec::Osaka, &config)
                .unwrap();
        let env = EthEvmConfig::new(Arc::new(spec)).evm_env(&RethHeader::default()).unwrap();
        assert_eq!(
            env.version().max_blobs_per_tx,
            usize::try_from(BlobParams::osaka().max_blobs_per_tx).unwrap()
        );
    }

    #[test]
    fn equal_timestamp_blob_overrides_follow_fork_order() {
        let mut config = FixtureConfig {
            blob_schedule: BTreeMap::from([
                (
                    ForkSpec::BPO2,
                    FixtureBlobParams {
                        target: U64::from(14),
                        max: U64::from(21),
                        base_fee_update_fraction: U128::from(5_007_716),
                    },
                ),
                (
                    ForkSpec::Amsterdam,
                    FixtureBlobParams {
                        target: U64::from(16),
                        max: U64::from(24),
                        base_fee_update_fraction: U128::from(5_007_716),
                    },
                ),
            ]),
            ..Default::default()
        };
        for fork in
            [ForkSpec::AmsterdamToBPOIncreaseAtTime15k, ForkSpec::AmsterdamToBPODecreaseAtTime15k]
        {
            let spec =
                fixture_chain_spec(&Header::default(), &State::default(), fork, &config).unwrap();
            let params = spec.blob_params_at_timestamp(14_999).unwrap();
            assert_eq!((params.target_blob_count, params.max_blob_count), (16, 24));
        }
        config.blob_schedule.remove(&ForkSpec::Amsterdam);
        let spec = fixture_chain_spec(
            &Header::default(),
            &State::default(),
            ForkSpec::AmsterdamToBPOIncreaseAtTime15k,
            &config,
        )
        .unwrap();
        assert_eq!(spec.blob_params_at_timestamp(0), Some(BlobParams::bpo2()));
    }

    #[test]
    fn experimental_merge_forks_are_not_executed_as_paris() {
        for fork in [ForkSpec::MergeEOF, ForkSpec::MergeMeterInitCode, ForkSpec::MergePush0] {
            let error = fixture_chain_spec(
                &Header::default(),
                &State::default(),
                fork,
                &FixtureConfig::default(),
            )
            .unwrap_err();
            assert!(matches!(error, FixtureConfigError::UnsupportedFork(actual) if actual == fork));
        }
    }

    #[test]
    fn genesis_quantities_return_errors_instead_of_panicking() {
        for field in [
            "gasLimit",
            "gasUsed",
            "number",
            "timestamp",
            "baseFeePerGas",
            "blobGasUsed",
            "excessBlobGas",
            "slotNumber",
        ] {
            let mut header = Header::default();
            let value = U256::from(u64::MAX) + U256::ONE;
            match field {
                "gasLimit" => header.gas_limit = value,
                "gasUsed" => header.gas_used = value,
                "number" => header.number = value,
                "timestamp" => header.timestamp = value,
                "baseFeePerGas" => header.base_fee_per_gas = Some(value),
                "blobGasUsed" => header.blob_gas_used = Some(value),
                "excessBlobGas" => header.excess_blob_gas = Some(value),
                "slotNumber" => header.slot_number = Some(value),
                _ => unreachable!(),
            }
            let error = fixture_chain_spec(
                &header,
                &State::default(),
                ForkSpec::Merge,
                &FixtureConfig::default(),
            )
            .unwrap_err();
            assert!(
                matches!(error, FixtureConfigError::HeaderQuantity {field: actual, ..} if actual == field)
            );
        }
    }

    #[test]
    fn unsafe_blob_schedules_return_configuration_errors() {
        for (target, max) in [(0, 0), (2, 1), (1, u64::MAX)] {
            let config = FixtureConfig {
                blob_schedule: BTreeMap::from([(
                    ForkSpec::Osaka,
                    FixtureBlobParams {
                        target: U64::from(target),
                        max: U64::from(max),
                        base_fee_update_fraction: U128::from(5_007_716),
                    },
                )]),
                ..Default::default()
            };
            let error =
                fixture_chain_spec(&Header::default(), &State::default(), ForkSpec::Osaka, &config)
                    .unwrap_err();
            assert!(
                matches!(error, FixtureConfigError::BlobParams {fork: ForkSpec::Osaka, target: actual_target, max: actual_max} if actual_target == target && actual_max == max)
            );
        }
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
    fn block_access_list_deserialize() {
        let valid = r#"{"rlp": "0xc0", "blockAccessList": [{"address": "0x01"}]}"#;
        let block = serde_json::from_str::<Block>(valid).unwrap();
        assert_eq!(block.access_list(), Some(&serde_json::json!([{"address": "0x01"}])));

        let invalid = r#"{
            "rlp": "0xc0",
            "expectException": "BlockException.INVALID_BLOCK_ACCESS_LIST",
            "rlp_decoded": {"blockAccessList": []}
        }"#;
        let block = serde_json::from_str::<Block>(invalid).unwrap();
        assert_eq!(block.access_list(), Some(&serde_json::json!([])));
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
