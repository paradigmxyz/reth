//! Reth genesis initialization utility functions.

use alloy_consensus::BlockHeader;
use alloy_genesis::GenesisAccount;
use alloy_primitives::{keccak256, map::HashMap, Address, Bytes, B256, KECCAK256_EMPTY, U256};
use reth_chainspec::EthChainSpec;
use reth_codecs::Compact;
use reth_config::config::EtlConfig;
use reth_db_api::{
    cursor::{DbCursorRW, DbDupCursorRO},
    table::{Decode, Decompress, Encode},
    tables,
    transaction::DbTxMut,
    DatabaseError,
};
use reth_etl::Collector;
use reth_execution_errors::StateRootError;
use reth_primitives_traits::{
    Account, Bytecode, GotExpected, NodePrimitives, SealedHeader, StorageEntry,
};
use reth_provider::{
    errors::provider::ProviderResult, providers::StaticFileWriter, BlockHashReader, BlockNumReader,
    BundleStateInit, ChainSpecProvider, DBProvider, DatabaseProviderFactory, ExecutionOutcome,
    HashingWriter, HeaderProvider, HistoryWriter, OriginalValuesKnown, ProviderError, RevertsInit,
    StageCheckpointReader, StageCheckpointWriter, StateWriter, StaticFileProviderFactory,
    TrieWriter,
};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_static_file_types::StaticFileSegment;
use reth_trie::{
    prefix_set::{TriePrefixSets, TriePrefixSetsMut},
    IntermediateStateRootState, Nibbles, StateRoot as StateRootComputer, StateRootProgress,
};
use reth_trie_db::DatabaseStateRoot;
use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::BTreeMap, io::BufRead};
use tracing::{debug, error, info, trace};

/// Default soft limit for number of bytes to read from state dump file, before inserting into
/// database.
///
/// Default is 1 GB.
pub const DEFAULT_SOFT_LIMIT_BYTE_LEN_ACCOUNTS_CHUNK: usize = 1_000_000_000;

/// Approximate number of accounts per 1 GB of state dump file. One account is approximately 3.5 KB
///
/// Approximate is 285 228 accounts.
//
// (14.05 GB OP mainnet state dump at Bedrock block / 4 007 565 accounts in file > 3.5 KB per
// account)
pub const AVERAGE_COUNT_ACCOUNTS_PER_GB_STATE_DUMP: usize = 285_228;

/// Soft limit for the number of flushed updates after which to log progress summary.
const SOFT_LIMIT_COUNT_FLUSHED_UPDATES: usize = 1_000_000;

/// Storage initialization error type.
#[derive(Debug, thiserror::Error, Clone)]
pub enum InitStorageError {
    /// Genesis header found on static files but the database is empty.
    #[error(
        "static files found, but the database is uninitialized. If attempting to re-syncing, delete both."
    )]
    UninitializedDatabase,
    /// An existing genesis block was found in the database, and its hash did not match the hash of
    /// the chainspec.
    #[error(
        "genesis hash in the storage does not match the specified chainspec: chainspec is {chainspec_hash}, database is {storage_hash}"
    )]
    GenesisHashMismatch {
        /// Expected genesis hash.
        chainspec_hash: B256,
        /// Actual genesis hash.
        storage_hash: B256,
    },
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// State root error while computing the state root
    #[error(transparent)]
    StateRootError(#[from] StateRootError),
    /// State root doesn't match the expected one.
    #[error("state root mismatch: {_0}")]
    StateRootMismatch(GotExpected<B256>),
}

impl From<DatabaseError> for InitStorageError {
    fn from(error: DatabaseError) -> Self {
        Self::Provider(ProviderError::Database(error))
    }
}

/// Write the genesis block if it has not already been written
pub fn init_genesis<PF>(factory: &PF) -> Result<B256, InitStorageError>
where
    PF: DatabaseProviderFactory
        + StaticFileProviderFactory<Primitives: NodePrimitives<BlockHeader: Compact>>
        + ChainSpecProvider
        + StageCheckpointReader
        + BlockHashReader,
    PF::ProviderRW: StaticFileProviderFactory<Primitives = PF::Primitives>
        + StageCheckpointWriter
        + HistoryWriter
        + HeaderProvider
        + HashingWriter
        + StateWriter
        + TrieWriter
        + AsRef<PF::ProviderRW>,
    PF::ChainSpec: EthChainSpec<Header = <PF::Primitives as NodePrimitives>::BlockHeader>,
{
    let chain = factory.chain_spec();

    let genesis = chain.genesis();
    let hash = chain.genesis_hash();

    // Check if we already have the genesis header or if we have the wrong one.
    match factory.block_hash(0) {
        Ok(None) | Err(ProviderError::MissingStaticFileBlock(StaticFileSegment::Headers, 0)) => {}
        Ok(Some(block_hash)) => {
            if block_hash == hash {
                // Some users will at times attempt to re-sync from scratch by just deleting the
                // database. Since `factory.block_hash` will only query the static files, we need to
                // make sure that our database has been written to, and throw error if it's empty.
                if factory.get_stage_checkpoint(StageId::Headers)?.is_none() {
                    error!(target: "reth::storage", "Genesis header found on static files, but database is uninitialized.");
                    return Err(InitStorageError::UninitializedDatabase);
                }

                debug!("Genesis already written, skipping.");
                return Ok(hash);
            }

            return Err(InitStorageError::GenesisHashMismatch {
                chainspec_hash: hash,
                storage_hash: block_hash,
            });
        }
        Err(e) => {
            debug!(?e);
            return Err(e.into());
        }
    }

    debug!("Writing genesis block.");

    let alloc = &genesis.alloc;

    // use transaction to insert genesis header
    let provider_rw = factory.database_provider_rw()?;
    insert_genesis_hashes(&provider_rw, alloc.iter())?;
    insert_genesis_history(&provider_rw, alloc.iter())?;

    // Insert header
    insert_genesis_header(&provider_rw, &chain)?;

    insert_genesis_state(&provider_rw, alloc.iter())?;

    // compute state root to populate trie tables
    compute_state_root(&provider_rw, None)?;

    // insert sync stage
    for stage in StageId::ALL {
        provider_rw.save_stage_checkpoint(stage, Default::default())?;
    }

    // Static file segments start empty, so we need to initialize the genesis block.
    let static_file_provider = provider_rw.static_file_provider();
    static_file_provider.latest_writer(StaticFileSegment::Receipts)?.increment_block(0)?;
    static_file_provider.latest_writer(StaticFileSegment::Transactions)?.increment_block(0)?;

    // `commit_unwind`` will first commit the DB and then the static file provider, which is
    // necessary on `init_genesis`.
    provider_rw.commit()?;

    Ok(hash)
}

/// Inserts the genesis state into the database.
pub fn insert_genesis_state<'a, 'b, Provider>(
    provider: &Provider,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)>,
) -> ProviderResult<()>
where
    Provider: StaticFileProviderFactory
        + DBProvider<Tx: DbTxMut>
        + HeaderProvider
        + StateWriter
        + AsRef<Provider>,
{
    insert_state(provider, alloc, 0)
}

/// Inserts state at given block into database.
pub fn insert_state<'a, 'b, Provider>(
    provider: &Provider,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)>,
    block: u64,
) -> ProviderResult<()>
where
    Provider: StaticFileProviderFactory
        + DBProvider<Tx: DbTxMut>
        + HeaderProvider
        + StateWriter
        + AsRef<Provider>,
{
    let capacity = alloc.size_hint().1.unwrap_or(0);
    let mut state_init: BundleStateInit =
        HashMap::with_capacity_and_hasher(capacity, Default::default());
    let mut reverts_init = HashMap::with_capacity_and_hasher(capacity, Default::default());
    let mut contracts: HashMap<B256, Bytecode> =
        HashMap::with_capacity_and_hasher(capacity, Default::default());

    for (address, account) in alloc {
        let bytecode_hash = if let Some(code) = &account.code {
            match Bytecode::new_raw_checked(code.clone()) {
                Ok(bytecode) => {
                    let hash = bytecode.hash_slow();
                    contracts.insert(hash, bytecode);
                    Some(hash)
                }
                Err(err) => {
                    error!(%address, %err, "Failed to decode genesis bytecode.");
                    return Err(DatabaseError::Other(err.to_string()).into());
                }
            }
        } else {
            None
        };

        // get state
        let storage = account
            .storage
            .as_ref()
            .map(|m| {
                m.iter()
                    .map(|(key, value)| {
                        let value = U256::from_be_bytes(value.0);
                        (*key, (U256::ZERO, value))
                    })
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();

        reverts_init.insert(
            *address,
            (Some(None), storage.keys().map(|k| StorageEntry::new(*k, U256::ZERO)).collect()),
        );

        state_init.insert(
            *address,
            (
                None,
                Some(Account {
                    nonce: account.nonce.unwrap_or_default(),
                    balance: account.balance,
                    bytecode_hash,
                }),
                storage,
            ),
        );
    }
    let all_reverts_init: RevertsInit = HashMap::from_iter([(block, reverts_init)]);

    let execution_outcome = ExecutionOutcome::new_init(
        state_init,
        all_reverts_init,
        contracts,
        Vec::default(),
        block,
        Vec::new(),
    );

    provider.write_state(&execution_outcome, OriginalValuesKnown::Yes)?;

    trace!(target: "reth::cli", "Inserted state");

    Ok(())
}

/// Inserts hashes for the genesis state.
pub fn insert_genesis_hashes<'a, 'b, Provider>(
    provider: &Provider,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)> + Clone,
) -> ProviderResult<()>
where
    Provider: DBProvider<Tx: DbTxMut> + HashingWriter,
{
    // insert and hash accounts to hashing table
    let alloc_accounts = alloc.clone().map(|(addr, account)| (*addr, Some(Account::from(account))));
    provider.insert_account_for_hashing(alloc_accounts)?;

    trace!(target: "reth::cli", "Inserted account hashes");

    let alloc_storage = alloc.filter_map(|(addr, account)| {
        // only return Some if there is storage
        account.storage.as_ref().map(|storage| {
            (*addr, storage.iter().map(|(&key, &value)| StorageEntry { key, value: value.into() }))
        })
    });
    provider.insert_storage_for_hashing(alloc_storage)?;

    trace!(target: "reth::cli", "Inserted storage hashes");

    Ok(())
}

/// Inserts history indices for genesis accounts and storage.
pub fn insert_genesis_history<'a, 'b, Provider>(
    provider: &Provider,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)> + Clone,
) -> ProviderResult<()>
where
    Provider: DBProvider<Tx: DbTxMut> + HistoryWriter,
{
    insert_history(provider, alloc, 0)
}

/// Inserts history indices for genesis accounts and storage.
pub fn insert_history<'a, 'b, Provider>(
    provider: &Provider,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)> + Clone,
    block: u64,
) -> ProviderResult<()>
where
    Provider: DBProvider<Tx: DbTxMut> + HistoryWriter,
{
    let account_transitions = alloc.clone().map(|(addr, _)| (*addr, [block]));
    provider.insert_account_history_index(account_transitions)?;

    trace!(target: "reth::cli", "Inserted account history");

    let storage_transitions = alloc
        .filter_map(|(addr, account)| account.storage.as_ref().map(|storage| (addr, storage)))
        .flat_map(|(addr, storage)| storage.keys().map(|key| ((*addr, *key), [block])));
    provider.insert_storage_history_index(storage_transitions)?;

    trace!(target: "reth::cli", "Inserted storage history");

    Ok(())
}

/// Inserts header for the genesis state.
pub fn insert_genesis_header<Provider, Spec>(
    provider: &Provider,
    chain: &Spec,
) -> ProviderResult<()>
where
    Provider: StaticFileProviderFactory<Primitives: NodePrimitives<BlockHeader: Compact>>
        + DBProvider<Tx: DbTxMut>,
    Spec: EthChainSpec<Header = <Provider::Primitives as NodePrimitives>::BlockHeader>,
{
    let (header, block_hash) = (chain.genesis_header(), chain.genesis_hash());
    let static_file_provider = provider.static_file_provider();

    match static_file_provider.block_hash(0) {
        Ok(None) | Err(ProviderError::MissingStaticFileBlock(StaticFileSegment::Headers, 0)) => {
            let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers)?;
            writer.append_header(header, &block_hash)?;
        }
        Ok(Some(_)) => {}
        Err(e) => return Err(e),
    }

    provider.tx_ref().put::<tables::HeaderNumbers>(block_hash, 0)?;
    provider.tx_ref().put::<tables::BlockBodyIndices>(0, Default::default())?;

    Ok(())
}

/// Reads account state from a [`BufRead`] reader and initializes it at the highest block that can
/// be found on database.
///
/// It's similar to [`init_genesis`] but supports importing state too big to fit in memory, and can
/// be set to the highest block present. One practical usecase is to import OP mainnet state at
/// bedrock transition block.
pub fn init_from_state_dump<Provider>(
    mut reader: impl BufRead,
    provider_rw: &Provider,
    etl_config: EtlConfig,
) -> eyre::Result<B256>
where
    Provider: StaticFileProviderFactory
        + DBProvider<Tx: DbTxMut>
        + BlockNumReader
        + BlockHashReader
        + ChainSpecProvider
        + StageCheckpointWriter
        + HistoryWriter
        + HeaderProvider
        + HashingWriter
        + TrieWriter
        + StateWriter
        + AsRef<Provider>,
{
    if etl_config.file_size == 0 {
        return Err(eyre::eyre!("ETL file size cannot be zero"));
    }

    let block = provider_rw.last_block_number()?;

    let hash = provider_rw
        .block_hash(block)?
        .ok_or_else(|| eyre::eyre!("Block hash not found for block {}", block))?;
    let header = provider_rw
        .header_by_number(block)?
        .map(SealedHeader::seal_slow)
        .ok_or_else(|| ProviderError::HeaderNotFound(block.into()))?;

    let expected_state_root = header.state_root();

    // first line can be state root
    let dump_state_root = parse_state_root(&mut reader)?;
    if expected_state_root != dump_state_root {
        error!(target: "reth::cli",
            ?dump_state_root,
            ?expected_state_root,
            header=?header.num_hash(),
            "State root from state dump does not match state root in current header."
        );
        return Err(InitStorageError::StateRootMismatch(GotExpected {
            got: dump_state_root,
            expected: expected_state_root,
        })
        .into());
    }

    debug!(target: "reth::cli",
        block,
        chain=%provider_rw.chain_spec().chain(),
        "Initializing state at block"
    );

    // remaining lines are accounts
    let parsed_accounts = parse_accounts(&mut reader, etl_config)?;

    // write state to db and collect prefix sets
    let mut prefix_sets = TriePrefixSetsMut::default();
    dump_state(parsed_accounts, provider_rw, block, &mut prefix_sets)?;

    info!(target: "reth::cli", "All accounts written to database, starting state root computation (may take some time)");

    // compute and compare state root. this advances the stage checkpoints.
    let computed_state_root = compute_state_root(provider_rw, Some(prefix_sets.freeze()))?;
    if computed_state_root == expected_state_root {
        info!(target: "reth::cli",
            ?computed_state_root,
            "Computed state root matches state root in state dump"
        );
    } else {
        error!(target: "reth::cli",
            ?computed_state_root,
            ?expected_state_root,
            "Computed state root does not match state root in state dump"
        );

        #[cfg(feature = "state-export")]
        {
            // Export computed state for debugging purposes
            info!(target: "reth::cli", "Exporting computed state for debugging...");
            if let Err(e) = export_state_on_mismatch(
                provider_rw,
                "computed_state_mismatch.json",
                Some(computed_state_root),
            ) {
                tracing::warn!(target: "reth::cli", error = ?e, "Failed to export computed state for debugging");
            }
        }
        return Err(InitStorageError::StateRootMismatch(GotExpected {
            got: computed_state_root,
            expected: expected_state_root,
        })
        .into());
    }

    // insert sync stages for stages that require state
    for stage in StageId::STATE_REQUIRED {
        provider_rw.save_stage_checkpoint(stage, StageCheckpoint::new(block))?;
    }

    Ok(hash)
}

/// Parses and returns expected state root.
fn parse_state_root(reader: &mut impl BufRead) -> eyre::Result<B256> {
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let expected_state_root = serde_json::from_str::<StateRoot>(&line)?.root;
    trace!(target: "reth::cli",
        root=%expected_state_root,
        "Read state root from file"
    );
    Ok(expected_state_root)
}

/// Parses accounts and pushes them to a [`Collector`].
fn parse_accounts(
    mut reader: impl BufRead,
    etl_config: EtlConfig,
) -> Result<ParsedAccounts, eyre::Error> {
    let mut line = String::new();
    let mut parsed = ParsedAccounts::new(etl_config);

    while let Ok(n) = reader.read_line(&mut line) {
        if n == 0 {
            break;
        }

        let GenesisAccountWithAddress {
            genesis_account,
            address,
            hashed_address,
            code_hash,
            storage_hashed,
        } = serde_json::from_str(&line)?;

        let hashed_address = resolved_hashed_address(address, hashed_address)?;
        let mut requires_hashed_snapshot_marker = address.is_none();

        // account is incomplete in dump (no address preimage), import directly into hashed tables.
        if address.is_none() {
            let account = dump_account_to_account(&genesis_account, code_hash)?;
            parsed.hashed_accounts.insert(hashed_address, account)?;
        }

        // State root is computed only from HashedStorages. Plain storage is written to
        // PlainStorageState by write_state, so we must also put plain slots into hashed_storage
        // for every account that has storage (with or without address).
        if let Some(storage) = genesis_account.storage.as_ref() {
            for (plain_key, value) in storage {
                parsed.hashed_storage.insert(
                    HashedStorageKey { hashed_address, hashed_slot: keccak256(*plain_key) },
                    *value,
                )?;
            }
        }

        if let Some(storage_hashed) = storage_hashed {
            let plain_hashed_storage = genesis_account.storage.as_ref().map(|storage| {
                storage
                    .iter()
                    .map(|(plain_key, value)| (keccak256(*plain_key), *value))
                    .collect::<HashMap<_, _>>()
            });

            for (hashed_key, hashed_value) in &storage_hashed {
                if let Some(plain_value) =
                    plain_hashed_storage.as_ref().and_then(|storage| storage.get(hashed_key)) &&
                    plain_value != hashed_value
                {
                    let account_id = address
                        .map(|addr| addr.to_string())
                        .unwrap_or_else(|| hashed_address.to_string());
                    return Err(eyre::eyre!(
                        "conflicting hashed/plain storage at account {account_id}: hashed_slot={hashed_key} plain_value={plain_value} hashed_value={hashed_value}"
                    ));
                }
            }

            if storage_hashed.keys().any(|hashed_key| {
                plain_hashed_storage
                    .as_ref()
                    .is_none_or(|storage| !storage.contains_key(hashed_key))
            }) {
                requires_hashed_snapshot_marker = true;
            }

            for (hashed_key, hashed_value) in storage_hashed {
                parsed.hashed_storage.insert(
                    HashedStorageKey { hashed_address, hashed_slot: hashed_key },
                    hashed_value,
                )?;
            }
        }

        if let Some(address) = address {
            parsed.accounts.insert(address, genesis_account)?;
        }

        parsed.requires_hashed_snapshot_marker |= requires_hashed_snapshot_marker;

        let parsed_total_accounts = parsed.accounts.len() + parsed.hashed_accounts.len();
        if parsed_total_accounts != 0 &&
            parsed_total_accounts.is_multiple_of(AVERAGE_COUNT_ACCOUNTS_PER_GB_STATE_DUMP)
        {
            info!(target: "reth::cli",
                parsed_new_accounts=parsed_total_accounts,
            );
        }

        line.clear();
    }

    Ok(parsed)
}
fn resolved_hashed_address(
    address: Option<Address>,
    hashed_address: Option<B256>,
) -> eyre::Result<B256> {
    match (address, hashed_address) {
        (Some(address), Some(hashed_address)) => {
            let computed = keccak256(address);
            if computed != hashed_address {
                return Err(eyre::eyre!(
                    "address/key mismatch: address={} key={} computed_key={}",
                    address,
                    hashed_address,
                    computed
                ));
            }
            Ok(hashed_address)
        }
        (Some(address), None) => Ok(keccak256(address)),
        (None, Some(hashed_address)) => Ok(hashed_address),
        (None, None) => Err(eyre::eyre!("state dump account must include either address or key")),
    }
}

fn dump_account_to_account(
    genesis_account: &GenesisAccount,
    code_hash: Option<B256>,
) -> eyre::Result<Account> {
    let bytecode_hash = if let Some(code) = &genesis_account.code {
        let bytecode = Bytecode::new_raw_checked(code.clone())
            .map_err(|err| eyre::eyre!("failed to decode dump bytecode: {err}"))?;
        let hash = bytecode.hash_slow();
        if let Some(expected_hash) = code_hash &&
            expected_hash != hash
        {
            return Err(eyre::eyre!(
                "account codeHash mismatch: expected {expected_hash}, computed {hash}"
            ));
        }
        Some(hash)
    } else {
        code_hash.filter(|hash| *hash != KECCAK256_EMPTY)
    };

    Ok(Account {
        nonce: genesis_account.nonce.unwrap_or_default(),
        balance: genesis_account.balance,
        bytecode_hash,
    })
}

/// Takes a [`Collector`] and processes all accounts.
fn dump_state<Provider>(
    mut parsed_accounts: ParsedAccounts,
    provider_rw: &Provider,
    block: u64,
    prefix_sets: &mut TriePrefixSetsMut,
) -> Result<(), eyre::Error>
where
    Provider: StaticFileProviderFactory
        + DBProvider<Tx: DbTxMut>
        + HeaderProvider
        + HashingWriter
        + HistoryWriter
        + StateWriter
        + AsRef<Provider>,
{
    let accounts_len = parsed_accounts.accounts.len();
    let requires_hashed_snapshot_marker = parsed_accounts.requires_hashed_snapshot_marker;
    let mut accounts = Vec::with_capacity(AVERAGE_COUNT_ACCOUNTS_PER_GB_STATE_DUMP);
    let mut total_inserted_accounts = 0;

    for (index, entry) in parsed_accounts.accounts.iter()?.enumerate() {
        let (address, account) = entry?;
        let (address, _) = Address::from_compact(address.as_slice(), address.len());
        let (account, _) = GenesisAccount::from_compact(account.as_slice(), account.len());

        // Add to prefix sets
        let hashed_address = keccak256(address);
        prefix_sets.account_prefix_set.insert(Nibbles::unpack(hashed_address));

        // Add storage keys to prefix sets if storage exists
        if let Some(ref storage) = account.storage {
            for key in storage.keys() {
                let hashed_key = keccak256(key);
                prefix_sets
                    .storage_prefix_sets
                    .entry(hashed_address)
                    .or_default()
                    .insert(Nibbles::unpack(hashed_key));
            }
        }

        accounts.push((address, account));

        if (index > 0 && index.is_multiple_of(AVERAGE_COUNT_ACCOUNTS_PER_GB_STATE_DUMP)) ||
            index == accounts_len - 1
        {
            total_inserted_accounts += accounts.len();

            info!(target: "reth::cli",
                total_inserted_accounts,
                "Writing accounts to db"
            );

            // use transaction to insert genesis header
            insert_genesis_hashes(
                provider_rw,
                accounts.iter().map(|(address, account)| (address, account)),
            )?;

            insert_history(
                provider_rw,
                accounts.iter().map(|(address, account)| (address, account)),
                block,
            )?;

            // block is already written to static files
            insert_state(
                provider_rw,
                accounts.iter().map(|(address, account)| (address, account)),
                block,
            )?;

            accounts.clear();
        }
    }
    insert_hashed_accounts(parsed_accounts.hashed_accounts, provider_rw, prefix_sets)?;

    insert_hashed_storage(parsed_accounts.hashed_storage, provider_rw, prefix_sets)?;
    if requires_hashed_snapshot_marker {
        write_hashed_snapshot_marker(provider_rw)?;
    }
    Ok(())
}

fn write_hashed_snapshot_marker<Provider>(provider_rw: &Provider) -> Result<(), eyre::Error>
where
    Provider: DBProvider<Tx: DbTxMut>,
{
    provider_rw.tx_ref().put::<tables::StageCheckpointProgresses>(
        tables::HASHED_SNAPSHOT_MARKER.to_string(),
        vec![1],
    )?;
    Ok(())
}

fn insert_hashed_accounts<Provider>(
    mut hashed_accounts: Collector<B256, Account>,
    provider_rw: &Provider,
    prefix_sets: &mut TriePrefixSetsMut,
) -> Result<(), eyre::Error>
where
    Provider: DBProvider<Tx: DbTxMut>,
{
    if hashed_accounts.is_empty() {
        return Ok(());
    }

    let mut cursor = provider_rw.tx_ref().cursor_write::<tables::HashedAccounts>()?;

    for entry in hashed_accounts.iter()? {
        let (key, value) = entry?;
        let hashed_address = B256::decode(&key)?;
        let account = Account::decompress(value.as_ref())?;

        prefix_sets.account_prefix_set.insert(Nibbles::unpack(hashed_address));
        cursor.upsert(hashed_address, &account)?;
    }

    Ok(())
}

fn insert_hashed_storage<Provider>(
    mut hashed_storage: Collector<HashedStorageKey, B256>,
    provider_rw: &Provider,
    prefix_sets: &mut TriePrefixSetsMut,
) -> Result<(), eyre::Error>
where
    Provider: DBProvider<Tx: DbTxMut>,
{
    if hashed_storage.is_empty() {
        return Ok(());
    }

    let mut hashed_storage_cursor =
        provider_rw.tx_ref().cursor_dup_write::<tables::HashedStorages>()?;

    for entry in hashed_storage.iter()? {
        let (key, value) = entry?;
        let key = HashedStorageKey::decode(&key)?;
        let value = B256::decompress(value.as_ref())?;

        let hashed_address = key.hashed_address;
        let hashed_key = key.hashed_slot;
        let value = U256::from_be_bytes(value.0);

        prefix_sets
            .storage_prefix_sets
            .entry(hashed_address)
            .or_default()
            .insert(Nibbles::unpack(hashed_key));

        if hashed_storage_cursor
            .seek_by_key_subkey(hashed_address, hashed_key)?
            .filter(|entry| entry.key == hashed_key)
            .is_some()
        {
            hashed_storage_cursor.delete_current()?;
        }

        if !value.is_zero() {
            hashed_storage_cursor
                .upsert(hashed_address, &StorageEntry { key: hashed_key, value })?;
        }
    }

    Ok(())
}

/// Computes the state root (from scratch) based on the accounts and storages present in the
/// database.
fn compute_state_root<Provider>(
    provider: &Provider,
    prefix_sets: Option<TriePrefixSets>,
) -> Result<B256, InitStorageError>
where
    Provider: DBProvider<Tx: DbTxMut> + TrieWriter,
{
    trace!(target: "reth::cli", "Computing state root");

    let tx = provider.tx_ref();
    let mut intermediate_state: Option<IntermediateStateRootState> = None;
    let mut total_flushed_updates = 0;

    loop {
        let mut state_root =
            StateRootComputer::from_tx(tx).with_intermediate_state(intermediate_state);

        if let Some(sets) = prefix_sets.clone() {
            state_root = state_root.with_prefix_sets(sets);
        }

        match state_root.root_with_progress()? {
            StateRootProgress::Progress(state, _, updates) => {
                let updated_len = provider.write_trie_updates(updates)?;
                total_flushed_updates += updated_len;

                trace!(target: "reth::cli",
                    last_account_key = %state.account_root_state.last_hashed_key,
                    updated_len,
                    total_flushed_updates,
                    "Flushing trie updates"
                );

                intermediate_state = Some(*state);

                if total_flushed_updates.is_multiple_of(SOFT_LIMIT_COUNT_FLUSHED_UPDATES) {
                    info!(target: "reth::cli",
                        total_flushed_updates,
                        "Flushing trie updates"
                    );
                }
            }
            StateRootProgress::Complete(root, _, updates) => {
                let updated_len = provider.write_trie_updates(updates)?;
                total_flushed_updates += updated_len;

                trace!(target: "reth::cli",
                    %root,
                    updated_len,
                    total_flushed_updates,
                    "State root has been computed"
                );

                return Ok(root);
            }
        }
    }
}

fn bytes_to_b256<'de, D>(bytes: Bytes) -> Result<B256, D::Error>
where
    D: Deserializer<'de>,
{
    if bytes.0.len() > 32 {
        return Err(serde::de::Error::custom("input too long to be a B256"));
    }

    // left-pad shorter values so exported short hex values (for example "01") are accepted.
    let mut padded = [0u8; 32];
    padded[32 - bytes.0.len()..].copy_from_slice(&bytes.0);
    Ok(B256::from_slice(&padded))
}

fn deserialize_hashed_storage_map<'de, D>(
    deserializer: D,
) -> Result<Option<BTreeMap<B256, B256>>, D::Error>
where
    D: Deserializer<'de>,
{
    if deserializer.is_human_readable() {
        let map = Option::<BTreeMap<Bytes, Bytes>>::deserialize(deserializer)?;
        match map {
            Some(map) => {
                let mut res_map = BTreeMap::new();
                for (key, value) in map {
                    let key = bytes_to_b256::<'de, D>(key)?;
                    let value = bytes_to_b256::<'de, D>(value)?;
                    res_map.insert(key, value);
                }
                Ok(Some(res_map))
            }
            None => Ok(None),
        }
    } else {
        Option::<BTreeMap<B256, B256>>::deserialize(deserializer)
    }
}

/// Type to deserialize state root from state dump file.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct StateRoot {
    root: B256,
}

/// An account as in the state dump file. This contains a [`GenesisAccount`] and the account's
/// address.
#[derive(Debug, Serialize, Deserialize)]
struct GenesisAccountWithAddress {
    /// The account's balance, nonce, code, and storage.
    #[serde(flatten)]
    genesis_account: GenesisAccount,
    /// The account's address preimage. Missing if preimage is unavailable.
    address: Option<Address>,
    /// Hashed account key (`keccak256(address)`) from state trie.
    #[serde(rename = "key", default)]
    hashed_address: Option<B256>,
    /// Optional code hash from dump line.
    #[serde(rename = "codeHash", default)]
    code_hash: Option<B256>,
    /// Storage entries keyed by already-hashed slot key.
    #[serde(default, deserialize_with = "deserialize_hashed_storage_map")]
    storage_hashed: Option<BTreeMap<B256, B256>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct HashedStorageKey {
    hashed_address: B256,
    hashed_slot: B256,
}

impl Encode for HashedStorageKey {
    type Encoded = [u8; 64];

    fn encode(self) -> Self::Encoded {
        let mut out = [0u8; 64];
        out[..32].copy_from_slice(self.hashed_address.as_slice());
        out[32..].copy_from_slice(self.hashed_slot.as_slice());
        out
    }
}

impl Decode for HashedStorageKey {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        if value.len() != 64 {
            return Err(DatabaseError::Decode);
        }
        Ok(Self {
            hashed_address: B256::from_slice(&value[..32]),
            hashed_slot: B256::from_slice(&value[32..]),
        })
    }
}

#[derive(Debug)]
struct ParsedAccounts {
    accounts: Collector<Address, GenesisAccount>,
    hashed_accounts: Collector<B256, Account>,
    hashed_storage: Collector<HashedStorageKey, B256>,
    requires_hashed_snapshot_marker: bool,
}

impl ParsedAccounts {
    fn new(etl_config: EtlConfig) -> Self {
        let EtlConfig { file_size, dir } = etl_config;
        Self {
            hashed_accounts: Collector::new(file_size, dir.clone()),
            accounts: Collector::new(file_size, dir.clone()),
            hashed_storage: Collector::new(file_size, dir),
            requires_hashed_snapshot_marker: false,
        }
    }
}

/// Export computed state for debugging when state root mismatch occurs.
///
/// This function exports all accounts and their storage from the database to a JSON file.
/// It's useful for debugging state root mismatches by comparing the computed state with
/// the expected state.
///
/// In the init stage, all state has been written to the database, so `bundle_state` is empty.
/// We pass `false` to export all account storage details from the database.
///
/// Only compiled when the `state-export` feature is enabled.
#[cfg(feature = "state-export")]
fn export_state_on_mismatch<Provider>(
    provider: &Provider,
    filename: &str,
    state_root: Option<B256>,
) -> eyre::Result<()>
where
    Provider: DBProvider<Tx: reth_db_api::transaction::DbTx>,
{
    use reth_mantle_forks::debug::state_export::export_full_state_with_bundle;
    use revm::database::BundleState;

    info!(target: "reth::cli", "Starting full state export to file: {}", filename);

    // In init stage, all state is in the database, bundle_state is empty
    let empty_bundle = BundleState::default();

    // Export with false to include all account storage (init scenario)
    export_full_state_with_bundle(provider, &empty_bundle, filename, state_root, false)?;

    info!(target: "reth::cli", "State export completed: {}", filename);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::constants::{
        HOLESKY_GENESIS_HASH, MAINNET_GENESIS_HASH, SEPOLIA_GENESIS_HASH,
    };
    use alloy_genesis::Genesis;
    #[cfg(feature = "state-export")]
    use alloy_primitives::map::HashMap;
    use reth_chainspec::{Chain, ChainSpec, HOLESKY, MAINNET, SEPOLIA};
    use reth_db::DatabaseEnv;
    use reth_db_api::{
        cursor::{DbCursorRO, DbDupCursorRO},
        models::{storage_sharded_key::StorageShardedKey, IntegerList, ShardedKey},
        table::{Decode, Decompress, Table, TableRow},
        transaction::DbTx,
        Database,
    };
    use reth_provider::{
        test_utils::{create_test_provider_factory_with_chain_spec, MockNodeTypesWithDB},
        ProviderFactory,
    };
    #[cfg(feature = "state-export")]
    use reth_trie::root::storage_root_unhashed;
    #[cfg(feature = "state-export")]
    use revm::state::AccountInfo;
    use std::{collections::BTreeMap, io::Cursor, sync::Arc};

    const HASHED_SNAPSHOT_MARKER: &str = "__hashed_only_state_snapshot__";

    fn collect_table_entries<DB, T>(
        tx: &<DB as Database>::TX,
    ) -> Result<Vec<TableRow<T>>, InitStorageError>
    where
        DB: Database,
        T: Table,
    {
        Ok(tx.cursor_read::<T>()?.walk_range(..)?.collect::<Result<Vec<_>, _>>()?)
    }

    #[test]
    fn success_init_genesis_mainnet() {
        let genesis_hash =
            init_genesis(&create_test_provider_factory_with_chain_spec(MAINNET.clone())).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, MAINNET_GENESIS_HASH);
    }

    #[test]
    fn success_init_genesis_sepolia() {
        let genesis_hash =
            init_genesis(&create_test_provider_factory_with_chain_spec(SEPOLIA.clone())).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, SEPOLIA_GENESIS_HASH);
    }

    #[test]
    fn success_init_genesis_holesky() {
        let genesis_hash =
            init_genesis(&create_test_provider_factory_with_chain_spec(HOLESKY.clone())).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, HOLESKY_GENESIS_HASH);
    }

    #[test]
    fn fail_init_inconsistent_db() {
        let factory = create_test_provider_factory_with_chain_spec(SEPOLIA.clone());
        let static_file_provider = factory.static_file_provider();
        init_genesis(&factory).unwrap();

        // Try to init db with a different genesis block
        let genesis_hash = init_genesis(&ProviderFactory::<MockNodeTypesWithDB>::new(
            factory.into_db(),
            MAINNET.clone(),
            static_file_provider,
        ));

        assert!(matches!(
            genesis_hash.unwrap_err(),
            InitStorageError::GenesisHashMismatch {
                chainspec_hash: MAINNET_GENESIS_HASH,
                storage_hash: SEPOLIA_GENESIS_HASH
            }
        ))
    }

    #[test]
    fn init_genesis_history() {
        let address_with_balance = Address::with_last_byte(1);
        let address_with_storage = Address::with_last_byte(2);
        let storage_key = B256::with_last_byte(1);
        let chain_spec = Arc::new(ChainSpec {
            chain: Chain::from_id(1),
            genesis: Genesis {
                alloc: BTreeMap::from([
                    (
                        address_with_balance,
                        GenesisAccount { balance: U256::from(1), ..Default::default() },
                    ),
                    (
                        address_with_storage,
                        GenesisAccount {
                            storage: Some(BTreeMap::from([(storage_key, B256::random())])),
                            ..Default::default()
                        },
                    ),
                ]),
                ..Default::default()
            },
            hardforks: Default::default(),
            paris_block_and_final_difficulty: None,
            deposit_contract: None,
            ..Default::default()
        });

        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        init_genesis(&factory).unwrap();

        let provider = factory.provider().unwrap();

        let tx = provider.tx_ref();

        assert_eq!(
            collect_table_entries::<Arc<DatabaseEnv>, tables::AccountsHistory>(tx)
                .expect("failed to collect"),
            vec![
                (ShardedKey::new(address_with_balance, u64::MAX), IntegerList::new([0]).unwrap()),
                (ShardedKey::new(address_with_storage, u64::MAX), IntegerList::new([0]).unwrap())
            ],
        );

        assert_eq!(
            collect_table_entries::<Arc<DatabaseEnv>, tables::StoragesHistory>(tx)
                .expect("failed to collect"),
            vec![(
                StorageShardedKey::new(address_with_storage, storage_key, u64::MAX),
                IntegerList::new([0]).unwrap()
            )],
        );
    }

    #[test]
    fn parse_accounts_collects_hashed_storage_slots() {
        let address = Address::with_last_byte(0xaa);
        let hashed_slot = B256::with_last_byte(0x11);
        let hashed_value = B256::with_last_byte(0x22);

        let line = format!(
            r#"{{"balance":"0","nonce":0,"root":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","codeHash":"0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470","address":"{address}","storage_hashed":{{"{hashed_slot}":"{hashed_value}"}}}}"#
        );

        let mut parsed =
            parse_accounts(Cursor::new(format!("{line}\n")), EtlConfig::new(None, 1024))
                .expect("failed to parse accounts");
        assert_eq!(parsed.accounts.len(), 1);
        assert_eq!(parsed.hashed_storage.len(), 1);

        let entries = parsed
            .hashed_storage
            .iter()
            .expect("iter")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        let (key, value) = entries.first().expect("entry");

        let decoded_key = HashedStorageKey::decode(key).expect("decode key");
        let decoded_value = B256::decompress(value.as_ref()).expect("decode value");

        assert_eq!(decoded_key.hashed_address, keccak256(address));
        assert_eq!(decoded_key.hashed_slot, hashed_slot);
        assert_eq!(decoded_value, hashed_value);
    }

    #[test]
    fn parse_accounts_accepts_cropped_hashed_storage_values() {
        let address = Address::with_last_byte(0xac);
        let hashed_slot = B256::with_last_byte(0x11);

        let line = format!(
            r#"{{"balance":"0","nonce":0,"root":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","codeHash":"0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470","address":"{address}","storage_hashed":{{"{hashed_slot}":"01"}}}}"#
        );

        let mut parsed =
            parse_accounts(Cursor::new(format!("{line}\n")), EtlConfig::new(None, 1024))
                .expect("failed to parse accounts");
        assert_eq!(parsed.hashed_storage.len(), 1);

        let entries = parsed
            .hashed_storage
            .iter()
            .expect("iter")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        let (_, value) = entries.first().expect("entry");
        let decoded_value = B256::decompress(value.as_ref()).expect("decode value");

        assert_eq!(decoded_value, B256::with_last_byte(0x01));
    }
    #[test]
    fn parse_accounts_accepts_missing_address_with_key() {
        let hashed_address = B256::with_last_byte(0xee);
        let line = format!(
            r#"{{"balance":"1","nonce":2,"root":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","codeHash":"0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470","key":"{hashed_address}"}}"#
        );

        let mut parsed =
            parse_accounts(Cursor::new(format!("{line}\n")), EtlConfig::new(None, 1024))
                .expect("missing-address account with key should parse");
        assert_eq!(parsed.accounts.len(), 0);
        assert_eq!(parsed.hashed_accounts.len(), 1);

        let entries = parsed
            .hashed_accounts
            .iter()
            .expect("iter")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        let (key, value) = entries.first().expect("entry");

        let decoded_key = B256::decode(key).expect("decode key");
        let decoded_account = Account::decompress(value.as_ref()).expect("decode account");

        assert_eq!(decoded_key, hashed_address);
        assert_eq!(decoded_account.nonce, 2);
        assert_eq!(decoded_account.balance, U256::from(1));
        assert_eq!(decoded_account.bytecode_hash, None);
    }

    #[test]
    fn parse_accounts_rejects_missing_address_and_key() {
        let line = r#"{"balance":"1","nonce":2,"root":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","codeHash":"0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"}"#;

        let err = parse_accounts(Cursor::new(format!("{line}\n")), EtlConfig::new(None, 1024))
            .unwrap_err();
        assert!(err.to_string().contains("either address or key"), "unexpected error: {err}");
    }

    #[test]
    fn parse_accounts_rejects_conflicting_hashed_and_plain_storage() {
        let address = Address::with_last_byte(0xbb);
        let plain_key = B256::with_last_byte(0x01);
        let plain_value = B256::with_last_byte(0x02);
        let hashed_key = keccak256(plain_key);
        let conflicting_value = B256::with_last_byte(0x03);

        let line = format!(
            r#"{{"balance":"0","nonce":0,"root":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","codeHash":"0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470","address":"{address}","storage":{{"{plain_key}":"{plain_value}"}},"storage_hashed":{{"{hashed_key}":"{conflicting_value}"}}}}"#
        );

        let err = parse_accounts(Cursor::new(format!("{line}\n")), EtlConfig::new(None, 1024))
            .unwrap_err();
        assert!(
            err.to_string().contains("conflicting hashed/plain storage"),
            "unexpected error: {err}"
        );
    }
    #[test]
    fn dump_state_imports_hashed_only_accounts() {
        let factory = create_test_provider_factory_with_chain_spec(SEPOLIA.clone());
        init_genesis(&factory).expect("init genesis");

        let hashed_address = B256::with_last_byte(0xdd);
        let hashed_slot = B256::with_last_byte(0x44);
        let line = format!(
            r#"{{"balance":"5","nonce":2,"root":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","codeHash":"0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470","key":"{hashed_address}","storage_hashed":{{"{hashed_slot}":"09"}}}}"#
        );

        let provider_rw = factory.database_provider_rw().expect("provider");
        let parsed =
            parse_accounts(Cursor::new(format!("{line}\n")), EtlConfig::new(None, 1024 * 1024))
                .expect("parse accounts");

        let mut prefix_sets = TriePrefixSetsMut::default();
        dump_state(parsed, &provider_rw, 1, &mut prefix_sets).expect("dump state");
        let _ = compute_state_root(&provider_rw, Some(prefix_sets.freeze())).expect("compute root");
        provider_rw.commit().expect("commit");

        let provider = factory.provider().expect("provider");
        let imported_account = provider
            .tx_ref()
            .get::<tables::HashedAccounts>(hashed_address)
            .expect("get hashed account")
            .expect("hashed account exists");
        assert_eq!(
            imported_account,
            Account { nonce: 2, balance: U256::from(5), bytecode_hash: None }
        );

        let mut storage_cursor =
            provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().expect("storage cursor");
        let entry = storage_cursor
            .seek_by_key_subkey(hashed_address, hashed_slot)
            .expect("seek storage")
            .expect("storage entry");
        assert_eq!(entry.key, hashed_slot);
        assert_eq!(entry.value, U256::from(9));
        assert_eq!(
            provider
                .tx_ref()
                .get::<tables::StageCheckpointProgresses>(HASHED_SNAPSHOT_MARKER.to_string())
                .expect("get hashed snapshot marker"),
            Some(vec![1])
        );
    }

    #[test]
    fn dump_state_imports_hashed_slots_and_affects_root() {
        fn import_and_root(state_line: &str) -> (ProviderFactory<MockNodeTypesWithDB>, B256) {
            let factory = create_test_provider_factory_with_chain_spec(SEPOLIA.clone());
            init_genesis(&factory).expect("init genesis");

            let provider_rw = factory.database_provider_rw().expect("provider");
            let parsed = parse_accounts(
                Cursor::new(format!("{state_line}\n")),
                EtlConfig::new(None, 1024 * 1024),
            )
            .expect("parse accounts");

            let mut prefix_sets = TriePrefixSetsMut::default();
            dump_state(parsed, &provider_rw, 1, &mut prefix_sets).expect("dump state");

            let root =
                compute_state_root(&provider_rw, Some(prefix_sets.freeze())).expect("compute root");
            provider_rw.commit().expect("commit");
            (factory, root)
        }

        let address = Address::with_last_byte(0xcd);
        let plain_key = B256::with_last_byte(0x01);
        let plain_value = B256::with_last_byte(0x02);
        let hashed_slot_without_preimage = B256::with_last_byte(0x99);

        let line_without_hashed = format!(
            r#"{{"balance":"0","nonce":1,"root":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","codeHash":"0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470","address":"{address}","storage":{{"{plain_key}":"{plain_value}"}}}}"#
        );
        let line_with_hashed = format!(
            r#"{{"balance":"0","nonce":1,"root":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","codeHash":"0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470","address":"{address}","storage":{{"{plain_key}":"{plain_value}"}},"storage_hashed":{{"{hashed_slot_without_preimage}":"07"}}}}"#
        );

        let (_, root_without_hashed) = import_and_root(&line_without_hashed);
        let (factory_with_hashed, root_with_hashed) = import_and_root(&line_with_hashed);

        assert_ne!(
            root_with_hashed, root_without_hashed,
            "hashed slot should affect the computed state root"
        );

        let provider = factory_with_hashed.provider().expect("provider");
        let mut cursor =
            provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().expect("cursor");

        let hashed_address = keccak256(address);
        let entry = cursor
            .seek_by_key_subkey(hashed_address, hashed_slot_without_preimage)
            .expect("seek")
            .expect("hashed slot entry");

        assert_eq!(entry.key, hashed_slot_without_preimage);
        assert_eq!(entry.value, U256::from(7));
    }

    #[cfg(feature = "state-export")]
    #[test]
    fn state_export_storage_hash_matches_bundle_overlay() {
        use reth_mantle_forks::debug::state_export::export_full_state_with_bundle;
        use revm::database::states::BundleState;

        let factory = create_test_provider_factory_with_chain_spec(SEPOLIA.clone());
        init_genesis(&factory).expect("init genesis");

        let provider_rw = factory.database_provider_rw().expect("provider");
        let address = Address::with_last_byte(0xaa);
        let account = Account { nonce: 1, balance: U256::from(2), bytecode_hash: None };
        let plain_slot = B256::with_last_byte(0x01);
        let db_value = U256::from(3);
        let bundle_value = U256::from(7);
        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(plain_slot);

        provider_rw
            .tx_ref()
            .put::<tables::PlainAccountState>(address, account)
            .expect("put plain account");
        provider_rw
            .tx_ref()
            .put::<tables::HashedAccounts>(hashed_address, account)
            .expect("put hashed account");
        provider_rw
            .tx_ref()
            .put::<tables::PlainStorageState>(
                address,
                StorageEntry { key: plain_slot, value: db_value },
            )
            .expect("put plain storage");
        provider_rw
            .tx_ref()
            .put::<tables::HashedStorages>(
                hashed_address,
                StorageEntry { key: hashed_slot, value: db_value },
            )
            .expect("put hashed storage");

        let bundle_state = BundleState::builder(1..=1)
            .state_present_account_info(
                address,
                AccountInfo {
                    balance: account.balance,
                    nonce: account.nonce,
                    code_hash: account.bytecode_hash.unwrap_or_default(),
                    code: None,
                },
            )
            .state_storage(
                address,
                HashMap::from_iter([(U256::from_be_bytes(plain_slot.0), (db_value, bundle_value))]),
            )
            .build();

        let expected_root = storage_root_unhashed(BTreeMap::from([(plain_slot, bundle_value)]));

        let filename = std::env::temp_dir().join(format!(
            "state-export-test-{}-{}.json",
            std::process::id(),
            hashed_address
        ));
        export_full_state_with_bundle(
            &provider_rw,
            &bundle_state,
            filename.to_str().expect("utf8 path"),
            None,
            true,
        )
        .expect("export state");

        let exported: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&filename).expect("read exported file"))
                .expect("parse exported json");
        std::fs::remove_file(&filename).expect("remove exported file");

        let address_key = format!("{:?}", address);
        let expected_storage_hash =
            format!("0x{}", alloy_primitives::hex::encode(expected_root.as_slice()));
        assert_eq!(
            exported["accounts"][&address_key]["storage_hash"].as_str(),
            Some(expected_storage_hash.as_str())
        );
    }
}
