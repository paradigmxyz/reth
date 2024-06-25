//! Reth genesis initialization utility functions.

use alloy_genesis::GenesisAccount;
use reth_chainspec::ChainSpec;
use reth_codecs::Compact;
use reth_config::config::EtlConfig;
use reth_db::tables;
use reth_db_api::{database::Database, transaction::DbTxMut, DatabaseError};
use reth_etl::Collector;
use reth_primitives::{
    Account, Address, Bytecode, Receipts, StaticFileSegment, StorageEntry, B256, U256,
};
use reth_provider::{
    bundle_state::{BundleStateInit, RevertsInit},
    errors::provider::ProviderResult,
    providers::{StaticFileProvider, StaticFileWriter},
    BlockHashReader, BlockNumReader, ChainSpecProvider, DatabaseProviderRW, ExecutionOutcome,
    HashingWriter, HistoryWriter, OriginalValuesKnown, ProviderError, ProviderFactory,
    StageCheckpointWriter, StateWriter, StaticFileProviderFactory,
};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_trie::{IntermediateStateRootState, StateRoot as StateRootComputer, StateRootProgress};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    io::BufRead,
    ops::DerefMut,
    sync::Arc,
};
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
    /// Computed state root doesn't match state root in state dump file.
    #[error(
        "state root mismatch, state dump: {expected_state_root}, computed: {computed_state_root}"
    )]
    SateRootMismatch {
        /// Expected state root.
        expected_state_root: B256,
        /// Actual state root.
        computed_state_root: B256,
    },
}

impl From<DatabaseError> for InitDatabaseError {
    fn from(error: DatabaseError) -> Self {
        Self::Provider(ProviderError::Database(error))
    }
}

/// Write the genesis block if it has not already been written
pub fn init_genesis<DB: Database>(factory: ProviderFactory<DB>) -> Result<B256, InitDatabaseError> {
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
    let provider_rw = factory.provider_rw()?;
    insert_genesis_hashes(&provider_rw, alloc.iter())?;
    insert_genesis_history(&provider_rw, alloc.iter())?;

    // Insert header
    let tx = provider_rw.tx_ref();
    let static_file_provider = factory.static_file_provider();
    insert_genesis_header::<DB>(tx, &static_file_provider, chain.clone())?;

    insert_genesis_state::<DB>(tx, alloc.len(), alloc.iter())?;

    // insert sync stage
    for stage in StageId::ALL {
        provider_rw.save_stage_checkpoint(stage, Default::default())?;
    }

    provider_rw.commit()?;
    static_file_provider.commit()?;

    Ok(hash)
}

/// Inserts the genesis state into the database.
pub fn insert_genesis_state<'a, 'b, DB: Database>(
    tx: &<DB as Database>::TXMut,
    capacity: usize,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)>,
) -> ProviderResult<()> {
    insert_state::<DB>(tx, capacity, alloc, 0)
}

/// Inserts state at given block into database.
pub fn insert_state<'a, 'b, DB: Database>(
    tx: &<DB as Database>::TXMut,
    capacity: usize,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)>,
    block: u64,
) -> ProviderResult<()> {
    let mut state_init: BundleStateInit = HashMap::with_capacity(capacity);
    let mut reverts_init = HashMap::with_capacity(capacity);
    let mut contracts: HashMap<B256, Bytecode> = HashMap::with_capacity(capacity);

    for (address, account) in alloc {
        let bytecode_hash = if let Some(code) = &account.code {
            let bytecode = Bytecode::new_raw(code.clone());
            let hash = bytecode.hash_slow();
            contracts.insert(hash, bytecode);
            Some(hash)
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
        contracts.into_iter().collect(),
        Receipts::default(),
        block,
        Vec::new(),
    );

    execution_outcome.write_to_storage(tx, None, OriginalValuesKnown::Yes)?;

    trace!(target: "reth::cli", "Inserted state");

    Ok(())
}

/// Inserts hashes for the genesis state.
pub fn insert_genesis_hashes<'a, 'b, DB: Database>(
    provider: &DatabaseProviderRW<DB>,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)> + Clone,
) -> ProviderResult<()> {
    // insert and hash accounts to hashing table
    let alloc_accounts =
        alloc.clone().map(|(addr, account)| (*addr, Some(Account::from_genesis_account(account))));
    provider.insert_account_for_hashing(alloc_accounts)?;

    trace!(target: "reth::cli", "Inserted account hashes");

    let alloc_storage = alloc.filter_map(|(addr, account)| {
        // only return Some if there is storage
        account.storage.as_ref().map(|storage| {
            (
                *addr,
                storage
                    .clone()
                    .into_iter()
                    .map(|(key, value)| StorageEntry { key, value: value.into() }),
            )
        })
    });
    provider.insert_storage_for_hashing(alloc_storage)?;

    trace!(target: "reth::cli", "Inserted storage hashes");

    Ok(())
}

/// Inserts history indices for genesis accounts and storage.
pub fn insert_genesis_history<'a, 'b, DB: Database>(
    provider: &DatabaseProviderRW<DB>,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)> + Clone,
) -> ProviderResult<()> {
    insert_history::<DB>(provider, alloc, 0)
}

/// Inserts history indices for genesis accounts and storage.
pub fn insert_history<'a, 'b, DB: Database>(
    provider: &DatabaseProviderRW<DB>,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)> + Clone,
    block: u64,
) -> ProviderResult<()> {
    let account_transitions =
        alloc.clone().map(|(addr, _)| (*addr, vec![block])).collect::<BTreeMap<_, _>>();
    provider.insert_account_history_index(account_transitions)?;

    trace!(target: "reth::cli", "Inserted account history");

    let storage_transitions = alloc
        .filter_map(|(addr, account)| account.storage.as_ref().map(|storage| (addr, storage)))
        .flat_map(|(addr, storage)| storage.iter().map(|(key, _)| ((*addr, *key), vec![block])))
        .collect::<BTreeMap<_, _>>();
    provider.insert_storage_history_index(storage_transitions)?;

    trace!(target: "reth::cli", "Inserted storage history");

    Ok(())
}

/// Inserts header for the genesis state.
pub fn insert_genesis_header<DB: Database>(
    tx: &<DB as Database>::TXMut,
    static_file_provider: &StaticFileProvider,
    chain: Arc<ChainSpec>,
) -> ProviderResult<()> {
    let (header, block_hash) = chain.sealed_genesis_header().split();

    match static_file_provider.block_hash(0) {
        Ok(None) | Err(ProviderError::MissingStaticFileBlock(StaticFileSegment::Headers, 0)) => {
            let (difficulty, hash) = (header.difficulty, block_hash);
            let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers)?;
            writer.append_header(header, difficulty, hash)?;
        }
        Ok(Some(_)) => {}
        Err(e) => return Err(e),
    }

    tx.put::<tables::HeaderNumbers>(block_hash, 0)?;
    tx.put::<tables::BlockBodyIndices>(0, Default::default())?;

    Ok(())
}

/// Reads account state from a [`BufRead`] reader and initializes it at the highest block that can
/// be found on database.
///
/// It's similar to [`init_genesis`] but supports importing state too big to fit in memory, and can
/// be set to the highest block present. One practical usecase is to import OP mainnet state at
/// bedrock transition block.
pub fn init_from_state_dump<DB: Database>(
    mut reader: impl BufRead,
    factory: ProviderFactory<DB>,
    etl_config: EtlConfig,
) -> eyre::Result<B256> {
    let block = factory.last_block_number()?;
    let hash = factory.block_hash(block)?.unwrap();

    debug!(target: "reth::cli",
        block,
        chain=%factory.chain_spec().chain,
        "Initializing state at block"
    );

    // first line can be state root, then it can be used for verifying against computed state root
    let expected_state_root = parse_state_root(&mut reader)?;

    // remaining lines are accounts
    let collector = parse_accounts(&mut reader, etl_config)?;

    // write state to db
    let mut provider_rw = factory.provider_rw()?;
    dump_state(collector, &mut provider_rw, block)?;

    // compute and compare state root. this advances the stage checkpoints.
    let computed_state_root = compute_state_root(&provider_rw)?;
    if computed_state_root != expected_state_root {
        error!(target: "reth::cli",
            ?computed_state_root,
            ?expected_state_root,
            "Computed state root does not match state root in state dump"
        );

        Err(InitDatabaseError::SateRootMismatch { expected_state_root, computed_state_root })?
    } else {
        info!(target: "reth::cli",
            ?computed_state_root,
            "Computed state root matches state root in state dump"
        );
    }

    // insert sync stages for stages that require state
    for stage in StageId::STATE_REQUIRED {
        provider_rw.save_stage_checkpoint(stage, StageCheckpoint::new(block))?;
    }

    provider_rw.commit()?;

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
fn dump_state<DB: Database>(
    mut collector: Collector<Address, GenesisAccount>,
    provider_rw: &mut DatabaseProviderRW<DB>,
    block: u64,
) -> Result<(), eyre::Error> {
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
            let tx = provider_rw.deref_mut().tx_mut();
            insert_state::<DB>(
                tx,
                accounts.len(),
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
fn compute_state_root<DB: Database>(provider: &DatabaseProviderRW<DB>) -> eyre::Result<B256> {
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
                let updates_len = updates.len();

                trace!(target: "reth::cli",
                    last_account_key = %state.last_account_key,
                    updates_len,
                    total_flushed_updates,
                    "Flushing trie updates"
                );

                intermediate_state = Some(*state);
                updates.flush(tx)?;

                total_flushed_updates += updates_len;

                if total_flushed_updates % SOFT_LIMIT_COUNT_FLUSHED_UPDATES == 0 {
                    info!(target: "reth::cli",
                        total_flushed_updates,
                        "Flushing trie updates"
                    );
                }
            }
            StateRootProgress::Complete(root, _, updates) => {
                let updates_len = updates.len();

                updates.flush(tx)?;

                total_flushed_updates += updates_len;

                trace!(target: "reth::cli",
                    %root,
                    updates_len = updates_len,
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
    use reth_chainspec::{Chain, GOERLI, MAINNET, SEPOLIA};
    use reth_db::DatabaseEnv;
    use reth_db_api::{
        cursor::DbCursorRO,
        models::{storage_sharded_key::StorageShardedKey, ShardedKey},
        table::{Table, TableRow},
        transaction::DbTx,
    };
    use reth_primitives::{GOERLI_GENESIS_HASH, MAINNET_GENESIS_HASH, SEPOLIA_GENESIS_HASH};
    use reth_primitives_traits::IntegerList;
    use reth_provider::test_utils::create_test_provider_factory_with_chain_spec;

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
            init_genesis(create_test_provider_factory_with_chain_spec(MAINNET.clone())).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, MAINNET_GENESIS_HASH);
    }

    #[test]
    fn success_init_genesis_goerli() {
        let genesis_hash =
            init_genesis(create_test_provider_factory_with_chain_spec(GOERLI.clone())).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, GOERLI_GENESIS_HASH);
    }

    #[test]
    fn success_init_genesis_sepolia() {
        let genesis_hash =
            init_genesis(create_test_provider_factory_with_chain_spec(SEPOLIA.clone())).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, SEPOLIA_GENESIS_HASH);
    }

    #[test]
    fn fail_init_inconsistent_db() {
        let factory = create_test_provider_factory_with_chain_spec(SEPOLIA.clone());
        let static_file_provider = factory.static_file_provider();
        init_genesis(factory.clone()).unwrap();

        // Try to init db with a different genesis block
        let genesis_hash = init_genesis(ProviderFactory::new(
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
            hardforks: BTreeMap::default(),
            genesis_hash: None,
            paris_block_and_final_difficulty: None,
            deposit_contract: None,
            ..Default::default()
        });

        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        init_genesis(factory.clone()).unwrap();

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
