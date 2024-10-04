//! Reth genesis initialization utility functions.

use alloy_genesis::GenesisAccount;
use alloy_primitives::{Address, B256, U256};
use reth_chainspec::EthChainSpec;
use reth_codecs::Compact;
use reth_config::config::EtlConfig;
use reth_db::tables;
use reth_db_api::{transaction::DbTxMut, DatabaseError};
use reth_etl::Collector;
use reth_primitives::{Account, Bytecode, GotExpected, Receipts, StaticFileSegment, StorageEntry};
use reth_provider::{
    errors::provider::ProviderResult,
    providers::{StaticFileProvider, StaticFileWriter},
    writer::UnifiedStorageWriter,
    BlockHashReader, BlockNumReader, BundleStateInit, ChainSpecProvider, DBProvider,
    DatabaseProviderFactory, ExecutionOutcome, HashingWriter, HeaderProvider, HistoryWriter,
    OriginalValuesKnown, ProviderError, RevertsInit, StageCheckpointWriter, StateChangeWriter,
    StateWriter, StaticFileProviderFactory, TrieWriter,
};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_trie::{IntermediateStateRootState, StateRoot as StateRootComputer, StateRootProgress};
use reth_trie_db::DatabaseStateRoot;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::BufRead};
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

/// Database initialization error type.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum InitDatabaseError {
    /// An existing genesis block was found in the database, and its hash did not match the hash of
    /// the chainspec.
    #[error("genesis hash in the database does not match the specified chainspec: chainspec is {chainspec_hash}, database is {database_hash}")]
    GenesisHashMismatch {
        /// Expected genesis hash.
        chainspec_hash: B256,
        /// Actual genesis hash.
        database_hash: B256,
    },
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// State root doesn't match the expected one.
    #[error("state root mismatch: {_0}")]
    StateRootMismatch(GotExpected<B256>),
}

impl From<DatabaseError> for InitDatabaseError {
    fn from(error: DatabaseError) -> Self {
        Self::Provider(ProviderError::Database(error))
    }
}

/// Write the genesis block if it has not already been written
pub fn init_genesis<PF>(factory: &PF) -> Result<B256, InitDatabaseError>
where
    PF: DatabaseProviderFactory + StaticFileProviderFactory + ChainSpecProvider + BlockHashReader,
    PF::ProviderRW: StageCheckpointWriter
        + HistoryWriter
        + HeaderProvider
        + HashingWriter
        + StateChangeWriter
        + AsRef<PF::ProviderRW>,
{
    let chain = factory.chain_spec();

    let genesis = chain.genesis();
    let hash = chain.genesis_hash();

    // Check if we already have the genesis header or if we have the wrong one.
    match factory.block_hash(0) {
        Ok(None) | Err(ProviderError::MissingStaticFileBlock(StaticFileSegment::Headers, 0)) => {}
        Ok(Some(block_hash)) => {
            if block_hash == hash {
                debug!("Genesis already written, skipping.");
                return Ok(hash)
            }

            return Err(InitDatabaseError::GenesisHashMismatch {
                chainspec_hash: hash,
                database_hash: block_hash,
            })
        }
        Err(e) => return Err(dbg!(e).into()),
    }

    debug!("Writing genesis block.");

    let alloc = &genesis.alloc;

    // use transaction to insert genesis header
    let provider_rw = factory.database_provider_rw()?;
    insert_genesis_hashes(&provider_rw, alloc.iter())?;
    insert_genesis_history(&provider_rw, alloc.iter())?;

    // Insert header
    let static_file_provider = factory.static_file_provider();
    insert_genesis_header(&provider_rw, &static_file_provider, &chain)?;

    insert_genesis_state(&provider_rw, alloc.iter())?;

    // insert sync stage
    for stage in StageId::ALL {
        provider_rw.save_stage_checkpoint(stage, Default::default())?;
    }

    // Static file segments start empty, so we need to initialize the genesis block.
    let segment = StaticFileSegment::Receipts;
    static_file_provider.latest_writer(segment)?.increment_block(0)?;

    let segment = StaticFileSegment::Transactions;
    static_file_provider.latest_writer(segment)?.increment_block(0)?;

    // `commit_unwind`` will first commit the DB and then the static file provider, which is
    // necessary on `init_genesis`.
    UnifiedStorageWriter::commit_unwind(provider_rw, static_file_provider)?;

    Ok(hash)
}

/// Inserts the genesis state into the database.
pub fn insert_genesis_state<'a, 'b, Provider>(
    provider: &Provider,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)>,
) -> ProviderResult<()>
where
    Provider: DBProvider<Tx: DbTxMut> + StateChangeWriter + HeaderProvider + AsRef<Provider>,
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
    Provider: DBProvider<Tx: DbTxMut> + StateChangeWriter + HeaderProvider + AsRef<Provider>,
{
    let capacity = alloc.size_hint().1.unwrap_or(0);
    let mut state_init: BundleStateInit = HashMap::with_capacity(capacity);
    let mut reverts_init = HashMap::with_capacity(capacity);
    let mut contracts: HashMap<B256, Bytecode> = HashMap::with_capacity(capacity);

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
    let all_reverts_init: RevertsInit = HashMap::from([(block, reverts_init)]);

    let execution_outcome = ExecutionOutcome::new_init(
        state_init,
        all_reverts_init,
        contracts,
        Receipts::default(),
        block,
        Vec::new(),
    );

    let mut storage_writer = UnifiedStorageWriter::from_database(&provider);
    storage_writer.write_to_storage(execution_outcome, OriginalValuesKnown::Yes)?;

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
        .flat_map(|(addr, storage)| storage.iter().map(|(key, _)| ((*addr, *key), [block])));
    provider.insert_storage_history_index(storage_transitions)?;

    trace!(target: "reth::cli", "Inserted storage history");

    Ok(())
}

/// Inserts header for the genesis state.
pub fn insert_genesis_header<Provider, Spec>(
    provider: &Provider,
    static_file_provider: &StaticFileProvider,
    chain: &Spec,
) -> ProviderResult<()>
where
    Provider: DBProvider<Tx: DbTxMut>,
    Spec: EthChainSpec,
{
    let (header, block_hash) = (chain.genesis_header(), chain.genesis_hash());

    match static_file_provider.block_hash(0) {
        Ok(None) | Err(ProviderError::MissingStaticFileBlock(StaticFileSegment::Headers, 0)) => {
            let (difficulty, hash) = (header.difficulty, block_hash);
            let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers)?;
            writer.append_header(header, difficulty, &hash)?;
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
    Provider: DBProvider<Tx: DbTxMut>
        + BlockNumReader
        + BlockHashReader
        + ChainSpecProvider
        + StageCheckpointWriter
        + HistoryWriter
        + HeaderProvider
        + HashingWriter
        + StateChangeWriter
        + TrieWriter
        + AsRef<Provider>,
{
    let block = provider_rw.last_block_number()?;
    let hash = provider_rw.block_hash(block)?.unwrap();
    let expected_state_root = provider_rw
        .header_by_number(block)?
        .ok_or_else(|| ProviderError::HeaderNotFound(block.into()))?
        .state_root;

    // first line can be state root
    let dump_state_root = parse_state_root(&mut reader)?;
    if expected_state_root != dump_state_root {
        error!(target: "reth::cli",
            ?dump_state_root,
            ?expected_state_root,
            "State root from state dump does not match state root in current header."
        );
        return Err(InitDatabaseError::StateRootMismatch(GotExpected {
            got: dump_state_root,
            expected: expected_state_root,
        })
        .into())
    }

    debug!(target: "reth::cli",
        block,
        chain=%provider_rw.chain_spec().chain(),
        "Initializing state at block"
    );

    // remaining lines are accounts
    let collector = parse_accounts(&mut reader, etl_config)?;

    // write state to db
    dump_state(collector, provider_rw, block)?;

    // compute and compare state root. this advances the stage checkpoints.
    let computed_state_root = compute_state_root(provider_rw)?;
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

        return Err(InitDatabaseError::StateRootMismatch(GotExpected {
            got: computed_state_root,
            expected: expected_state_root,
        })
        .into())
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
) -> Result<Collector<Address, GenesisAccount>, eyre::Error> {
    let mut line = String::new();
    let mut collector = Collector::new(etl_config.file_size, etl_config.dir);

    while let Ok(n) = reader.read_line(&mut line) {
        if n == 0 {
            break
        }

        let GenesisAccountWithAddress { genesis_account, address } = serde_json::from_str(&line)?;
        collector.insert(address, genesis_account)?;

        if !collector.is_empty() && collector.len() % AVERAGE_COUNT_ACCOUNTS_PER_GB_STATE_DUMP == 0
        {
            info!(target: "reth::cli",
                parsed_new_accounts=collector.len(),
            );
        }

        line.clear();
    }

    Ok(collector)
}

/// Takes a [`Collector`] and processes all accounts.
fn dump_state<Provider>(
    mut collector: Collector<Address, GenesisAccount>,
    provider_rw: &Provider,
    block: u64,
) -> Result<(), eyre::Error>
where
    Provider: DBProvider<Tx: DbTxMut>
        + HeaderProvider
        + HashingWriter
        + HistoryWriter
        + StateChangeWriter
        + AsRef<Provider>,
{
    let accounts_len = collector.len();
    let mut accounts = Vec::with_capacity(AVERAGE_COUNT_ACCOUNTS_PER_GB_STATE_DUMP);
    let mut total_inserted_accounts = 0;

    for (index, entry) in collector.iter()?.enumerate() {
        let (address, account) = entry?;
        let (address, _) = Address::from_compact(address.as_slice(), address.len());
        let (account, _) = GenesisAccount::from_compact(account.as_slice(), account.len());

        accounts.push((address, account));

        if (index > 0 && index % AVERAGE_COUNT_ACCOUNTS_PER_GB_STATE_DUMP == 0) ||
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
    Ok(())
}

/// Computes the state root (from scratch) based on the accounts and storages present in the
/// database.
fn compute_state_root<Provider>(provider: &Provider) -> eyre::Result<B256>
where
    Provider: DBProvider<Tx: DbTxMut> + TrieWriter,
{
    trace!(target: "reth::cli", "Computing state root");

    let tx = provider.tx_ref();
    let mut intermediate_state: Option<IntermediateStateRootState> = None;
    let mut total_flushed_updates = 0;

    loop {
        match StateRootComputer::from_tx(tx)
            .with_intermediate_state(intermediate_state)
            .root_with_progress()?
        {
            StateRootProgress::Progress(state, _, updates) => {
                let updated_len = provider.write_trie_updates(&updates)?;
                total_flushed_updates += updated_len;

                trace!(target: "reth::cli",
                    last_account_key = %state.last_account_key,
                    updated_len,
                    total_flushed_updates,
                    "Flushing trie updates"
                );

                intermediate_state = Some(*state);

                if total_flushed_updates % SOFT_LIMIT_COUNT_FLUSHED_UPDATES == 0 {
                    info!(target: "reth::cli",
                        total_flushed_updates,
                        "Flushing trie updates"
                    );
                }
            }
            StateRootProgress::Complete(root, _, updates) => {
                let updated_len = provider.write_trie_updates(&updates)?;
                total_flushed_updates += updated_len;

                trace!(target: "reth::cli",
                    %root,
                    updated_len,
                    total_flushed_updates,
                    "State root has been computed"
                );

                return Ok(root)
            }
        }
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
    /// The account's address.
    address: Address,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_genesis::Genesis;
    use reth_chainspec::{Chain, ChainSpec, HOLESKY, MAINNET, SEPOLIA};
    use reth_db::DatabaseEnv;
    use reth_db_api::{
        cursor::DbCursorRO,
        models::{storage_sharded_key::StorageShardedKey, ShardedKey},
        table::{Table, TableRow},
        transaction::DbTx,
        Database,
    };
    use reth_primitives::{HOLESKY_GENESIS_HASH, MAINNET_GENESIS_HASH, SEPOLIA_GENESIS_HASH};
    use reth_primitives_traits::IntegerList;
    use reth_provider::{
        test_utils::{create_test_provider_factory_with_chain_spec, MockNodeTypesWithDB},
        ProviderFactory,
    };
    use std::{collections::BTreeMap, sync::Arc};

    fn collect_table_entries<DB, T>(
        tx: &<DB as Database>::TX,
    ) -> Result<Vec<TableRow<T>>, InitDatabaseError>
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

        assert_eq!(
            genesis_hash.unwrap_err(),
            InitDatabaseError::GenesisHashMismatch {
                chainspec_hash: MAINNET_GENESIS_HASH,
                database_hash: SEPOLIA_GENESIS_HASH
            }
        )
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
            genesis_hash: Default::default(),
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
}
