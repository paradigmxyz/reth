//! Command exporting the latest canonical state as a JSONL dump.

use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use alloy_consensus::{constants::KECCAK_EMPTY, BlockHeader as AlloyBlockHeader};
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use alloy_rlp::Encodable;
use clap::Parser;
use eyre::{eyre, Result};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_db::BlockNumberList;
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    models::{storage_sharded_key::StorageShardedKey, ShardedKey},
    transaction::DbTx,
    AccountsHistory, Bytecodes, HashedAccounts, HashedStorages, PlainAccountState,
    PlainStorageState, StoragesHistory,
};
use reth_node_api::NodePrimitives;
use reth_primitives_traits::Account;
use reth_provider::{
    providers::RocksDBProvider, AccountReader, BlockHashReader, BlockNumReader, DBProvider,
    HeaderProvider, ProviderResult, PruneCheckpointReader, RocksDBProviderFactory,
    StorageSettingsCache,
};
use reth_prune_types::PruneSegment;
use reth_tasks::Runtime;
use serde::Serialize;
use std::{
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::info;

/// Log export progress every N accounts.
const LOG_INTERVAL_ACCOUNTS: usize = 100_000;
/// Poll long-running storage loops every N items to decide whether to emit a timed progress log.
const LOG_POLL_STORAGE_INTERVAL: usize = 10_000;
/// Emit a timed progress log at most once per interval while exporting a single large account.
const LOG_INTERVAL_DURATION: Duration = Duration::from_secs(30);

/// Export the latest canonical state as a JSONL dump.
#[derive(Debug, Parser)]
pub struct ExportStateCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    /// Optional file where the latest canonical header will be written as raw RLP bytes.
    ///
    /// This can be passed to `reth init-state --without-evm --header <HEADER_FILE>` for
    /// non-genesis imports.
    #[arg(long, value_name = "HEADER_FILE", verbatim_doc_comment)]
    header: Option<PathBuf>,

    /// Destination JSONL file for the exported state dump.
    #[arg(value_name = "STATE_DUMP_FILE", verbatim_doc_comment)]
    output: PathBuf,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> ExportStateCommand<C> {
    /// Execute `export-state` command.
    pub async fn execute<N>(self, runtime: Runtime) -> Result<()>
    where
        N: CliNodeTypes<
            ChainSpec = C::ChainSpec,
            Primitives: NodePrimitives<BlockHeader: AlloyBlockHeader + Encodable>,
        >,
    {
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RO, runtime)?;
        let provider = provider_factory.provider()?;

        let block_number = provider.last_block_number()?;
        let block_hash = provider
            .block_hash(block_number)?
            .ok_or_else(|| eyre!("Block hash not found for block {block_number}"))?;
        let header = provider
            .header_by_number(block_number)?
            .ok_or_else(|| eyre!("Header not found for block {block_number}"))?;
        let state_root = header.state_root();

        if let Some(path) = self.header.as_deref() {
            write_parent_dir(path)?;
            reth_fs_util::write(path, alloy_rlp::encode(&header))?;
            info!(
                target: "reth::cli",
                block = block_number,
                hash = ?block_hash,
                path = %path.display(),
                "Wrote header sidecar"
            );
        }

        write_parent_dir(&self.output)?;
        let file = reth_fs_util::create_file(&self.output)?;
        let mut writer = CountingWriter::new(BufWriter::new(file));

        let rocksdb = provider_factory.rocksdb_provider();
        let counters = export_state_dump(&provider, &rocksdb, state_root, &mut writer)?;
        writer.flush()?;

        info!(
            target: "reth::cli",
            block = block_number,
            hash = ?block_hash,
            state_root = ?state_root,
            accounts = counters.accounts,
            storage_slots = counters.storage_slots,
            bytes_written = writer.bytes_written(),
            output = %self.output.display(),
            "State export complete"
        );

        Ok(())
    }
}

impl<C: ChainSpecParser> ExportStateCommand<C> {
    /// Returns the underlying chain being used to run this command.
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ExportCounters {
    accounts: usize,
    storage_slots: usize,
}

trait StateDumpWriter: Write {
    fn bytes_written(&self) -> u64;
}

struct CountingWriter<W> {
    inner: W,
    bytes_written: u64,
}

impl<W> CountingWriter<W> {
    const fn new(inner: W) -> Self {
        Self { inner, bytes_written: 0 }
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.bytes_written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }

    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.inner.write_all(buf)?;
        self.bytes_written += buf.len() as u64;
        Ok(())
    }
}

impl<W: Write> StateDumpWriter for CountingWriter<W> {
    fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

impl StateDumpWriter for Vec<u8> {
    fn bytes_written(&self) -> u64 {
        self.len() as u64
    }
}

struct ProgressReporter {
    last_log: Instant,
}

struct HashedStateProgress {
    address: Address,
    history_slots: usize,
    db_slots_scanned: usize,
    written_slots: usize,
    phase: &'static str,
}

impl ProgressReporter {
    fn new() -> Self {
        Self { last_log: Instant::now() }
    }

    fn maybe_log_hashed_state<W: StateDumpWriter>(
        &mut self,
        writer: &W,
        counters: ExportCounters,
        progress: HashedStateProgress,
    ) {
        if self.last_log.elapsed() < LOG_INTERVAL_DURATION {
            return
        }

        self.last_log = Instant::now();
        info!(
            target: "reth::cli",
            accounts = counters.accounts,
            storage_slots = counters.storage_slots,
            bytes_written = writer.bytes_written(),
            current_address = %progress.address,
            current_account_history_slots = progress.history_slots,
            current_account_db_slots_scanned = progress.db_slots_scanned,
            current_account_written_slots = progress.written_slots,
            phase = progress.phase,
            "Exporting hashed state"
        );
    }
}

#[derive(Debug, Serialize)]
struct StateRootLine {
    root: B256,
}

struct UniqueAccountHistoryIter<I> {
    inner: I,
    last: Option<Address>,
}

impl<I> UniqueAccountHistoryIter<I> {
    const fn new(inner: I) -> Self {
        Self { inner, last: None }
    }
}

impl<I> Iterator for UniqueAccountHistoryIter<I>
where
    I: Iterator<Item = ProviderResult<(ShardedKey<Address>, BlockNumberList)>>,
{
    type Item = ProviderResult<Address>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let next = match self.inner.next()? {
                Ok(next) => next,
                Err(err) => return Some(Err(err)),
            };
            let (key, _) = next;
            if self.last == Some(key.key) {
                continue;
            }
            self.last = Some(key.key);
            return Some(Ok(key.key));
        }
    }
}

struct UniqueStorageHistoryIter<I> {
    inner: I,
    last: Option<(Address, B256)>,
}

impl<I> UniqueStorageHistoryIter<I> {
    const fn new(inner: I) -> Self {
        Self { inner, last: None }
    }
}

impl<I> Iterator for UniqueStorageHistoryIter<I>
where
    I: Iterator<Item = ProviderResult<(StorageShardedKey, BlockNumberList)>>,
{
    type Item = ProviderResult<(Address, B256)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let next = match self.inner.next()? {
                Ok(next) => next,
                Err(err) => return Some(Err(err)),
            };
            let (key, _) = next;
            let current = (key.address, key.sharded_key.key);
            if self.last == Some(current) {
                continue;
            }
            self.last = Some(current);
            return Some(Ok(current));
        }
    }
}

fn write_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        reth_fs_util::create_dir_all(parent)?;
    }
    Ok(())
}

fn export_state_dump<Provider, W>(
    provider: &Provider,
    rocksdb: &RocksDBProvider,
    state_root: B256,
    writer: &mut W,
) -> Result<ExportCounters>
where
    Provider: AccountReader + PruneCheckpointReader + StorageSettingsCache + DBProvider<Tx: DbTx>,
    W: StateDumpWriter,
{
    serde_json::to_writer(&mut *writer, &StateRootLine { root: state_root })?;
    writer.write_all(b"\n")?;

    if provider.cached_storage_settings().use_hashed_state() {
        export_hashed_state(provider, rocksdb, writer)
    } else {
        export_plain_state(provider, writer)
    }
}

fn export_plain_state<Provider, W>(provider: &Provider, writer: &mut W) -> Result<ExportCounters>
where
    Provider: DBProvider<Tx: DbTx>,
    W: StateDumpWriter,
{
    let tx = provider.tx_ref();
    let mut accounts_cursor = tx.cursor_read::<PlainAccountState>()?;
    let mut storage_cursor = tx.cursor_dup_read::<PlainStorageState>()?;
    let mut counters = ExportCounters::default();

    for entry in accounts_cursor.walk(None)? {
        let (address, account) = entry?;
        let code = load_account_code(provider, &account)?;

        write_account_prefix(writer, &account, code.as_ref())?;

        let mut first_storage_entry = true;
        for storage_entry in storage_cursor.walk_dup(Some(address), None)? {
            let (_, storage_entry) = storage_entry?;
            if storage_entry.value == U256::ZERO {
                continue;
            }

            write_storage_entry(
                writer,
                &mut first_storage_entry,
                storage_entry.key,
                B256::from(storage_entry.value.to_be_bytes()),
            )?;
            counters.storage_slots += 1;
        }

        finish_account_line(writer, address, !first_storage_entry)?;
        counters.accounts += 1;

        if counters.accounts.is_multiple_of(LOG_INTERVAL_ACCOUNTS) {
            info!(
                target: "reth::cli",
                accounts = counters.accounts,
                storage_slots = counters.storage_slots,
                bytes_written = writer.bytes_written(),
                "Exporting plain state"
            );
        }
    }

    Ok(counters)
}

fn export_hashed_state<Provider, W>(
    provider: &Provider,
    rocksdb: &RocksDBProvider,
    writer: &mut W,
) -> Result<ExportCounters>
where
    Provider: AccountReader + PruneCheckpointReader + DBProvider<Tx: DbTx>,
    W: StateDumpWriter,
{
    ensure_history_available(provider, rocksdb)?;

    let mut account_history = UniqueAccountHistoryIter::new(rocksdb.iter::<AccountsHistory>()?);
    let mut storage_history = UniqueStorageHistoryIter::new(rocksdb.iter::<StoragesHistory>()?);
    let mut next_storage = storage_history.next().transpose()?;
    let mut hashed_storage_cursor = provider.tx_ref().cursor_dup_read::<HashedStorages>()?;
    let mut counters = ExportCounters::default();
    let mut progress = ProgressReporter::new();

    for address in &mut account_history {
        let address = address?;

        let Some(account) = provider.basic_account(&address)? else {
            skip_account_storage_history(
                &mut storage_history,
                &mut next_storage,
                address,
                writer,
                counters,
                &mut progress,
            )?;
            continue;
        };

        let code = load_account_code(provider, &account)?;
        write_account_prefix(writer, &account, code.as_ref())?;

        let mut first_storage_entry = true;
        let account_storage_slots = collect_account_storage_slots(
            &mut storage_history,
            &mut next_storage,
            address,
            writer,
            counters,
            &mut progress,
        )?;
        let account_history_slots = account_storage_slots.len();
        write_current_hashed_account_storage(
            &mut hashed_storage_cursor,
            writer,
            address,
            account_storage_slots,
            &mut first_storage_entry,
            &mut counters,
            &mut progress,
        )?;

        finish_account_line(writer, address, !first_storage_entry)?;
        counters.accounts += 1;

        if counters.accounts.is_multiple_of(LOG_INTERVAL_ACCOUNTS) {
            info!(
                target: "reth::cli",
                accounts = counters.accounts,
                storage_slots = counters.storage_slots,
                bytes_written = writer.bytes_written(),
                current_address = %address,
                current_account_history_slots = account_history_slots,
                "Exporting hashed state"
            );
        }
    }

    if let Some((storage_address, slot_key)) = next_storage {
        return Err(eyre!(
            "storage history is inconsistent: leftover slot {slot_key} for address {storage_address}"
        ));
    }

    Ok(counters)
}

fn skip_account_storage_history<I, W>(
    storage_history: &mut UniqueStorageHistoryIter<I>,
    next_storage: &mut Option<(Address, B256)>,
    address: Address,
    writer: &W,
    counters: ExportCounters,
    progress: &mut ProgressReporter,
) -> Result<()>
where
    I: Iterator<Item = ProviderResult<(StorageShardedKey, BlockNumberList)>>,
    W: StateDumpWriter,
{
    let mut skipped_slots = 0usize;

    while let Some((storage_address, slot_key)) = next_storage.as_ref().copied() {
        if storage_address < address {
            return Err(eyre!(
                "storage history is inconsistent: found slot {slot_key} for {storage_address} before its account entry"
            ));
        }
        if storage_address > address {
            break;
        }

        skipped_slots += 1;
        if skipped_slots.is_multiple_of(LOG_POLL_STORAGE_INTERVAL) {
            progress.maybe_log_hashed_state(
                writer,
                counters,
                HashedStateProgress {
                    address,
                    history_slots: skipped_slots,
                    db_slots_scanned: 0,
                    written_slots: 0,
                    phase: "skipping stale storage history",
                },
            );
        }

        *next_storage = storage_history.next().transpose()?;
    }

    Ok(())
}

fn collect_account_storage_slots<I, W>(
    storage_history: &mut UniqueStorageHistoryIter<I>,
    next_storage: &mut Option<(Address, B256)>,
    address: Address,
    writer: &W,
    counters: ExportCounters,
    progress: &mut ProgressReporter,
) -> Result<Vec<(B256, B256)>>
where
    I: Iterator<Item = ProviderResult<(StorageShardedKey, BlockNumberList)>>,
    W: StateDumpWriter,
{
    let mut account_storage_slots = Vec::new();

    while let Some((storage_address, slot_key)) = next_storage.as_ref().copied() {
        if storage_address < address {
            return Err(eyre!(
                "storage history is inconsistent: found slot {slot_key} for {storage_address} before its account entry"
            ));
        }
        if storage_address > address {
            break;
        }

        account_storage_slots.push((keccak256(slot_key), slot_key));
        if account_storage_slots.len().is_multiple_of(LOG_POLL_STORAGE_INTERVAL) {
            progress.maybe_log_hashed_state(
                writer,
                counters,
                HashedStateProgress {
                    address,
                    history_slots: account_storage_slots.len(),
                    db_slots_scanned: 0,
                    written_slots: 0,
                    phase: "collecting storage history",
                },
            );
        }

        *next_storage = storage_history.next().transpose()?;
    }

    account_storage_slots.sort_unstable_by_key(|(hashed_slot, _)| *hashed_slot);
    Ok(account_storage_slots)
}

fn write_current_hashed_account_storage<CURSOR, W>(
    hashed_storage_cursor: &mut CURSOR,
    writer: &mut W,
    address: Address,
    account_storage_slots: Vec<(B256, B256)>,
    first_storage_entry: &mut bool,
    counters: &mut ExportCounters,
    progress: &mut ProgressReporter,
) -> Result<()>
where
    CURSOR: DbCursorRO<HashedStorages> + DbDupCursorRO<HashedStorages>,
    W: StateDumpWriter,
{
    let hashed_address = keccak256(address);

    if account_storage_slots.is_empty() {
        if hashed_storage_cursor.seek_exact(hashed_address)?.is_some() {
            return Err(eyre!(
                "storage history is inconsistent: live storage for address {address} is missing history entries"
            ));
        }
        return Ok(())
    }

    let mut account_storage_slots = account_storage_slots.into_iter().peekable();
    let history_slots = account_storage_slots.len();
    let mut db_slots_scanned = 0usize;
    let mut written_slots = 0usize;

    for storage_entry in hashed_storage_cursor.walk_dup(Some(hashed_address), None)? {
        let (_, storage_entry) = storage_entry?;
        db_slots_scanned += 1;

        if db_slots_scanned.is_multiple_of(LOG_POLL_STORAGE_INTERVAL) {
            progress.maybe_log_hashed_state(
                writer,
                *counters,
                HashedStateProgress {
                    address,
                    history_slots,
                    db_slots_scanned,
                    written_slots,
                    phase: "walking current hashed storage",
                },
            );
        }

        loop {
            let Some((hashed_slot, plain_slot)) = account_storage_slots.peek().copied() else {
                if storage_entry.value != U256::ZERO {
                    return Err(eyre!(
                        "storage history is inconsistent: live hashed slot {} for address {address} is missing history entry",
                        storage_entry.key
                    ));
                }
                break
            };

            match hashed_slot.cmp(&storage_entry.key) {
                std::cmp::Ordering::Less => {
                    account_storage_slots.next();
                }
                std::cmp::Ordering::Equal => {
                    if storage_entry.value != U256::ZERO {
                        write_storage_entry(
                            writer,
                            first_storage_entry,
                            plain_slot,
                            B256::from(storage_entry.value.to_be_bytes()),
                        )?;
                        counters.storage_slots += 1;
                        written_slots += 1;
                    }

                    account_storage_slots.next();
                    break;
                }
                std::cmp::Ordering::Greater => {
                    if storage_entry.value != U256::ZERO {
                        return Err(eyre!(
                            "storage history is inconsistent: live hashed slot {} for address {address} is missing history entry",
                            storage_entry.key
                        ));
                    }
                    break
                }
            }
        }
    }

    Ok(())
}

fn ensure_history_available<Provider>(provider: &Provider, rocksdb: &RocksDBProvider) -> Result<()>
where
    Provider: PruneCheckpointReader + DBProvider<Tx: DbTx>,
{
    for segment in [PruneSegment::AccountHistory, PruneSegment::StorageHistory] {
        if provider
            .get_prune_checkpoint(segment)?
            .is_some_and(|checkpoint| checkpoint.block_number.is_some())
        {
            return Err(eyre!(
                "cannot export state from a storage_v2 database with pruned {segment} history"
            ));
        }
    }

    if provider.tx_ref().entries::<HashedAccounts>()? > 0 &&
        rocksdb.first::<AccountsHistory>()?.is_none()
    {
        return Err(eyre!(
            "cannot export state from a storage_v2 database without account history indices"
        ));
    }

    if provider.tx_ref().entries::<HashedStorages>()? > 0 &&
        rocksdb.first::<StoragesHistory>()?.is_none()
    {
        return Err(eyre!(
            "cannot export state from a storage_v2 database without storage history indices"
        ));
    }

    Ok(())
}

fn load_account_code<Provider>(provider: &Provider, account: &Account) -> Result<Option<Bytes>>
where
    Provider: DBProvider<Tx: DbTx>,
{
    let Some(bytecode_hash) = account.bytecode_hash else {
        return Ok(None);
    };
    if bytecode_hash == KECCAK_EMPTY {
        return Ok(None);
    }

    let code = provider
        .tx_ref()
        .get::<Bytecodes>(bytecode_hash)?
        .ok_or_else(|| eyre!("Missing bytecode for hash {bytecode_hash}"))?;
    Ok(Some(code.original_bytes()))
}

fn write_account_prefix<W: Write>(
    writer: &mut W,
    account: &Account,
    code: Option<&Bytes>,
) -> Result<()> {
    writer.write_all(br#"{"balance":"#)?;
    serde_json::to_writer(&mut *writer, &account.balance)?;
    writer.write_all(br#","nonce":"#)?;
    serde_json::to_writer(&mut *writer, &account.nonce)?;

    if let Some(code) = code {
        writer.write_all(br#","code":"#)?;
        serde_json::to_writer(&mut *writer, code)?;
    }

    Ok(())
}

fn write_storage_entry<W: Write>(
    writer: &mut W,
    first_storage_entry: &mut bool,
    key: B256,
    value: B256,
) -> Result<()> {
    if *first_storage_entry {
        writer.write_all(br#","storage":{"#)?;
        *first_storage_entry = false;
    } else {
        writer.write_all(b",")?;
    }

    serde_json::to_writer(&mut *writer, &key)?;
    writer.write_all(b":")?;
    serde_json::to_writer(&mut *writer, &value)?;
    Ok(())
}

fn finish_account_line<W: Write>(
    writer: &mut W,
    address: Address,
    wrote_storage: bool,
) -> Result<()> {
    if wrote_storage {
        writer.write_all(b"}")?;
    }

    writer.write_all(br#","address":"#)?;
    serde_json::to_writer(&mut *writer, &address)?;
    writer.write_all(b"}\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_state::without_evm::read_header_from_file;
    use alloy_consensus::Header;
    use alloy_genesis::{Genesis, GenesisAccount};
    use clap::Parser;
    use reth_chainspec::{Chain, ChainSpec};
    use reth_db_common::init::init_genesis_with_settings;
    use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
    use reth_provider::{
        test_utils::{create_test_provider_factory_with_chain_spec, MockNodeTypesWithDB},
        DatabaseProviderFactory, ProviderFactory, PruneCheckpointWriter, StorageSettings,
    };
    use reth_prune_types::{PruneCheckpoint, PruneMode};
    use std::collections::BTreeMap;
    use tempfile::NamedTempFile;

    #[test]
    fn parse_export_state_command() {
        let cmd: ExportStateCommand<EthereumChainSpecParser> = ExportStateCommand::parse_from([
            "reth",
            "--chain",
            "sepolia",
            "--header",
            "header.rlp",
            "state.jsonl",
        ]);

        assert_eq!(cmd.output.to_str().unwrap(), "state.jsonl");
        assert_eq!(cmd.header.unwrap().to_str().unwrap(), "header.rlp");
    }

    #[test]
    fn export_state_v1_includes_accounts_and_storage() {
        let chain_spec = test_chain_spec();
        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        init_genesis_with_settings(&factory, StorageSettings::v1()).unwrap();

        let dump = export_dump(&factory).unwrap();
        let lines: Vec<_> = String::from_utf8(dump).unwrap().lines().map(str::to_owned).collect();

        assert_eq!(lines.len(), 3);

        let account_lines: Vec<serde_json::Value> =
            lines[1..].iter().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert!(account_lines.iter().any(|line| {
            line.get("address") ==
                Some(&serde_json::Value::String(Address::with_last_byte(1).to_string()))
        }));
        assert!(account_lines.iter().any(|line| {
            line.get("address") ==
                Some(&serde_json::Value::String(Address::with_last_byte(2).to_string())) &&
                line.get("storage").is_some()
        }));
    }

    #[test]
    fn export_state_v2_uses_history_indices() {
        let chain_spec = test_chain_spec();
        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        init_genesis_with_settings(&factory, StorageSettings::v2()).unwrap();

        let dump = export_dump(&factory).unwrap();
        let lines: Vec<_> = String::from_utf8(dump).unwrap().lines().map(str::to_owned).collect();

        assert_eq!(lines.len(), 3);

        let root_line: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(root_line.get("root").is_some());

        let account_lines: Vec<serde_json::Value> =
            lines[1..].iter().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert!(account_lines.iter().any(|line| {
            line.get("address") ==
                Some(&serde_json::Value::String(Address::with_last_byte(1).to_string()))
        }));
        assert!(account_lines.iter().any(|line| {
            line.get("address") ==
                Some(&serde_json::Value::String(Address::with_last_byte(2).to_string()))
        }));
    }

    #[test]
    fn export_state_v2_rejects_pruned_history() {
        let chain_spec = test_chain_spec();
        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        init_genesis_with_settings(&factory, StorageSettings::v2()).unwrap();

        let provider_rw = factory.database_provider_rw().unwrap();
        provider_rw
            .save_prune_checkpoint(
                PruneSegment::AccountHistory,
                PruneCheckpoint {
                    block_number: Some(0),
                    tx_number: None,
                    prune_mode: PruneMode::Full,
                },
            )
            .unwrap();
        provider_rw.commit().unwrap();

        let err = export_dump(&factory).unwrap_err();
        assert!(err.to_string().contains("pruned AccountHistory history"));
    }

    #[test]
    fn export_state_v2_rejects_missing_all_live_storage_history_for_account() {
        let chain_spec = test_chain_spec_two_storage_accounts();
        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        init_genesis_with_settings(&factory, StorageSettings::v2()).unwrap();

        let address = Address::with_last_byte(3);
        factory
            .rocksdb_provider()
            .delete::<StoragesHistory>(StorageShardedKey::last(address, B256::with_last_byte(0x01)))
            .unwrap();

        let err = export_dump(&factory).unwrap_err();
        assert!(err.to_string().contains("missing history entries"));
    }

    #[test]
    fn export_state_v2_rejects_missing_live_storage_history() {
        let chain_spec = test_chain_spec_two_storage_slots();
        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        init_genesis_with_settings(&factory, StorageSettings::v2()).unwrap();

        let address = Address::with_last_byte(2);
        let removed_slot = B256::with_last_byte(0x02);
        factory
            .rocksdb_provider()
            .delete::<StoragesHistory>(StorageShardedKey::last(address, removed_slot))
            .unwrap();

        let err = export_dump(&factory).unwrap_err();
        assert!(err.to_string().contains("missing history entry"));
    }

    #[test]
    fn exported_header_sidecar_is_readable() {
        let chain_spec = test_chain_spec();
        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        init_genesis_with_settings(&factory, StorageSettings::v1()).unwrap();

        let provider = factory.provider().unwrap();
        let block_number = provider.last_block_number().unwrap();
        let header = provider.header_by_number(block_number).unwrap().unwrap();

        let file = NamedTempFile::new().unwrap();
        reth_fs_util::write(file.path(), alloy_rlp::encode(&header)).unwrap();

        let decoded: Header = read_header_from_file(file.path()).unwrap();
        assert_eq!(decoded.hash_slow(), header.hash_slow());
    }

    fn export_dump(factory: &ProviderFactory<MockNodeTypesWithDB>) -> Result<Vec<u8>> {
        let provider = factory.provider()?;
        let block_number = provider.last_block_number()?;
        let header = provider
            .header_by_number(block_number)?
            .ok_or_else(|| eyre!("Header not found for block {block_number}"))?;
        let mut output = Vec::new();
        export_state_dump(
            &provider,
            &factory.rocksdb_provider(),
            header.state_root(),
            &mut output,
        )?;
        Ok(output)
    }

    fn test_chain_spec() -> Arc<ChainSpec> {
        let code = Bytes::from_static(&[0x60, 0x00, 0x60, 0x00, 0x55]);
        let storage_key = B256::with_last_byte(0x01);
        let storage_value = B256::with_last_byte(0x02);

        Arc::new(ChainSpec {
            chain: Chain::from_id(1),
            genesis: Genesis {
                alloc: BTreeMap::from([
                    (
                        Address::with_last_byte(1),
                        GenesisAccount::default().with_balance(U256::from(100_u64)),
                    ),
                    (
                        Address::with_last_byte(2),
                        GenesisAccount::default()
                            .with_balance(U256::from(200_u64))
                            .with_nonce(Some(7))
                            .with_code(Some(code))
                            .with_storage(Some(BTreeMap::from([(storage_key, storage_value)]))),
                    ),
                ]),
                ..Default::default()
            },
            hardforks: Default::default(),
            paris_block_and_final_difficulty: None,
            deposit_contract: None,
            ..Default::default()
        })
    }

    fn test_chain_spec_two_storage_slots() -> Arc<ChainSpec> {
        let code = Bytes::from_static(&[0x60, 0x00, 0x60, 0x00, 0x55]);
        let storage_key_1 = B256::with_last_byte(0x01);
        let storage_value_1 = B256::with_last_byte(0x02);
        let storage_key_2 = B256::with_last_byte(0x02);
        let storage_value_2 = B256::with_last_byte(0x03);

        Arc::new(ChainSpec {
            chain: Chain::from_id(1),
            genesis: Genesis {
                alloc: BTreeMap::from([
                    (
                        Address::with_last_byte(1),
                        GenesisAccount::default().with_balance(U256::from(100_u64)),
                    ),
                    (
                        Address::with_last_byte(2),
                        GenesisAccount::default()
                            .with_balance(U256::from(200_u64))
                            .with_nonce(Some(7))
                            .with_code(Some(code))
                            .with_storage(Some(BTreeMap::from([
                                (storage_key_1, storage_value_1),
                                (storage_key_2, storage_value_2),
                            ]))),
                    ),
                ]),
                ..Default::default()
            },
            hardforks: Default::default(),
            paris_block_and_final_difficulty: None,
            deposit_contract: None,
            ..Default::default()
        })
    }

    fn test_chain_spec_two_storage_accounts() -> Arc<ChainSpec> {
        let code = Bytes::from_static(&[0x60, 0x00, 0x60, 0x00, 0x55]);
        let storage_key = B256::with_last_byte(0x01);

        Arc::new(ChainSpec {
            chain: Chain::from_id(1),
            genesis: Genesis {
                alloc: BTreeMap::from([
                    (
                        Address::with_last_byte(1),
                        GenesisAccount::default().with_balance(U256::from(100_u64)),
                    ),
                    (
                        Address::with_last_byte(2),
                        GenesisAccount::default().with_balance(U256::from(200_u64)).with_storage(
                            Some(BTreeMap::from([(storage_key, B256::with_last_byte(0x02))])),
                        ),
                    ),
                    (
                        Address::with_last_byte(3),
                        GenesisAccount::default()
                            .with_balance(U256::from(300_u64))
                            .with_nonce(Some(7))
                            .with_code(Some(code))
                            .with_storage(Some(BTreeMap::from([(
                                storage_key,
                                B256::with_last_byte(0x03),
                            )]))),
                    ),
                ]),
                ..Default::default()
            },
            hardforks: Default::default(),
            paris_block_and_final_difficulty: None,
            deposit_contract: None,
            ..Default::default()
        })
    }
}
