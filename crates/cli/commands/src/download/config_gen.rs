use crate::download::{
    manifest::{ComponentManifest, ComponentSelection, SnapshotComponentType, SnapshotManifest},
    SelectionPreset,
};
use reth_chainspec::{EthereumHardfork, EthereumHardforks};
use reth_config::config::{BlocksPerFileConfig, Config, PruneConfig, StaticFilesConfig};
use reth_db::{mdbx, models::StorageSettings, tables, DatabaseError};
use reth_db_api::{
    cursor::DbCursorRO,
    table::Table,
    transaction::{DbTx, DbTxMut},
};
use reth_node_core::args::DefaultPruningValues;
use reth_prune_types::{
    PruneCheckpoint, PruneMode, PruneSegment, MINIMUM_DISTANCE, MINIMUM_UNWIND_SAFE_DISTANCE,
};
use reth_stages_types::StageCheckpoint;
use reth_storage_api::metadata::keys;
use std::{collections::BTreeMap, path::Path};
use tracing::info;

/// Writes a [`Config`] as TOML to `<data_dir>/reth.toml`.
///
/// If the file already exists, it is not overwritten. Returns `true` if the file was written.
pub fn write_config(config: &Config, data_dir: &Path) -> eyre::Result<bool> {
    let config_path = data_dir.join("reth.toml");

    if config_path.exists() {
        info!(target: "reth::cli",
            path = ?config_path,
            "reth.toml already exists, skipping config generation"
        );
        return Ok(false);
    }

    let toml_str = toml::to_string_pretty(config)?;
    reth_fs_util::write(&config_path, toml_str)?;

    info!(target: "reth::cli",
        path = ?config_path,
        "Generated reth.toml based on downloaded components"
    );

    Ok(true)
}

/// Writes prune checkpoints for the restored data with the configured retention modes.
pub(super) fn write_prune_checkpoints_tx<Tx>(
    tx: &Tx,
    config: &Config,
    selections: &BTreeMap<SnapshotComponentType, ComponentSelection>,
    manifest: &SnapshotManifest,
    is_v2: bool,
    explicit_history: bool,
) -> eyre::Result<()>
where
    Tx: DbTx + DbTxMut,
{
    let segments = &config.prune.segments;

    for (segment, component, mode) in [
        (
            PruneSegment::SenderRecovery,
            SnapshotComponentType::TransactionSenders,
            segments.sender_recovery,
        ),
        (
            PruneSegment::TransactionLookup,
            SnapshotComponentType::RocksdbIndices,
            segments.transaction_lookup,
        ),
        (PruneSegment::Receipts, SnapshotComponentType::Receipts, segments.receipts),
        (
            PruneSegment::AccountHistory,
            SnapshotComponentType::AccountChangesets,
            segments.account_history,
        ),
        (
            PruneSegment::StorageHistory,
            SnapshotComponentType::StorageChangesets,
            segments.storage_history,
        ),
        (PruneSegment::Bodies, SnapshotComponentType::Transactions, segments.bodies_history),
    ] {
        if segment == PruneSegment::TransactionLookup && !is_v2 {
            continue
        }
        let history =
            matches!(segment, PruneSegment::AccountHistory | PruneSegment::StorageHistory);
        let mut target = match selections.get(&component) {
            Some(ComponentSelection::All) => None,
            Some(ComponentSelection::Distance(distance)) => manifest.block.checked_sub(*distance),
            Some(ComponentSelection::Since(block)) => {
                block.checked_sub(1).map(|n| n.min(manifest.block))
            }
            Some(ComponentSelection::None) | None => Some(manifest.block),
        };
        let existing = tx.get::<tables::PruneCheckpoints>(segment)?;
        if !is_v2 && history {
            if !explicit_history {
                continue
            }
            // V1 always carries its changesets in the state archive.
            target = None;
        } else if manifest.component(component).is_some() {
            // A lagging component may have no archive covering the snapshot tip.
            if manifest.chunks_for_distance(component, Some(1)) == 0 {
                target = Some(manifest.block);
            }
        } else if target.is_some() {
            // Pruned receipts and legacy senders can live entirely in the state archive.
            let bundled = match segment {
                PruneSegment::Receipts => {
                    has_bundled_transactions::<tables::Receipts>(tx, existing, manifest.block)?
                }
                PruneSegment::SenderRecovery if !is_v2 => has_bundled_transactions::<
                    tables::TransactionSenders,
                >(
                    tx, existing, manifest.block
                )?,
                _ => false,
            };
            if bundled {
                target = None;
            }
        }
        let (block_number, tx_number) = match (target, existing) {
            (None, Some(source)) => (source.block_number, source.tx_number),
            // Bodies prune whole files by block, so a policy can be recorded before pruning starts.
            (None, None) if segment == PruneSegment::Bodies && mode.is_some() => (None, None),
            (None, None) => continue,
            (Some(target), source) if history => {
                let block =
                    source.and_then(|cp| cp.block_number).map_or(target, |block| block.max(target));
                (Some(block), None)
            }
            (Some(target), Some(source)) if source.block_number > Some(target) => {
                (source.block_number, source.tx_number)
            }
            (Some(target), source) => {
                let tx_number = tx
                    .get::<tables::BlockBodyIndices>(target)?
                    .and_then(|indices| indices.next_tx_num().checked_sub(1));
                (Some(target), tx_number.max(source.and_then(|cp| cp.tx_number)))
            }
        };
        // Unlimited retention preserves the restore boundary without a rolling window.
        let prune_mode = mode.unwrap_or_else(|| {
            PruneMode::Before(block_number.map_or(0, |block| block.saturating_add(1)))
        });
        let checkpoint = PruneCheckpoint { block_number, tx_number, prune_mode };
        tx.put::<tables::PruneCheckpoints>(segment, checkpoint)?;

        info!(target: "reth::cli",
            segment = %segment,
            block = ?checkpoint.block_number,
            tx = ?checkpoint.tx_number,
            mode = ?checkpoint.prune_mode,
            "Set prune checkpoint"
        );
    }

    Ok(())
}

/// Returns whether the bundled table contains every transaction after the source checkpoint.
/// Receipt log filtering can leave gaps, so finding a surviving row is insufficient.
fn has_bundled_transactions<T: Table<Key = u64>>(
    tx: &impl DbTx,
    source: Option<PruneCheckpoint>,
    tip: u64,
) -> eyre::Result<bool> {
    let Some(body) = tx.get::<tables::BlockBodyIndices>(tip)? else { return Ok(false) };
    let Some(last_tx) = body.next_tx_num().checked_sub(1) else { return Ok(true) };
    let mut cursor = tx.cursor_read::<tables::RawTable<T>>()?;
    let (Some((first, _)), Some((last, _))) = (cursor.first()?, cursor.last()?) else {
        return Ok(false)
    };
    let (first, last) = (first.key()?, last.key()?);
    let start_tx = match source {
        Some(PruneCheckpoint { block_number: Some(block), .. }) => {
            tx.get::<tables::BlockBodyIndices>(block)?.map_or(0, |indices| indices.next_tx_num())
        }
        _ => 0,
    };
    Ok(first <= start_tx &&
        last >= last_tx &&
        (last - first).checked_add(1) == Some(u64::try_from(tx.entries::<T>()?)?))
}

/// Reads the snapshot layout before updating the checkpoints that depend on it.
pub(super) fn snapshot_uses_v2(
    tx: &impl DbTx,
    selections: &BTreeMap<SnapshotComponentType, ComponentSelection>,
) -> eyre::Result<bool> {
    let bytes = match tx.get::<tables::Metadata>(keys::STORAGE_SETTINGS.to_owned()) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok(false),
        // Snapshots predating storage settings can also lack the metadata table itself.
        Err(DatabaseError::Open(err)) if err == mdbx::Error::NotFound.into() => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    if let Ok(settings) = serde_json::from_slice::<StorageSettings>(&bytes) {
        return Ok(settings.is_v2())
    }

    // The node defaults to v1 for unrecognized settings. Some old releases explicitly placed
    // tables on backends the v1 reader cannot use.
    if let Ok(flags) = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes)
    {
        for (name, value) in flags {
            if value.as_bool() != Some(true) {
                continue
            }
            let incompatible = match name.as_str() {
                "transaction_senders_in_static_files" => matches!(
                    selections.get(&SnapshotComponentType::TransactionSenders),
                    Some(selection) if *selection != ComponentSelection::None
                ),
                "storages_history_in_rocksdb" |
                "transaction_hash_numbers_in_rocksdb" |
                "account_history_in_rocksdb" |
                "account_changesets_in_static_files" |
                "storage_changesets_in_static_files" |
                "use_hashed_state" => true,
                _ => false,
            };
            eyre::ensure!(
                !incompatible,
                "Unsupported legacy snapshot setting for this download: {name}=true"
            );
        }
    }
    Ok(false)
}

/// Stages backed by the optional RocksDB index component in a v2 snapshot.
const INDEX_STAGE_IDS: [&str; 3] =
    ["TransactionLookup", "IndexAccountHistory", "IndexStorageHistory"];

/// Prune segments whose checkpoints only describe the rocksdb indices.
///
/// `AccountHistory` and `StorageHistory` checkpoints are kept: they also record where the
/// downloaded changesets start, which history lookups need to fall back to plain state for
/// accounts and slots unchanged since before that block.
const INDEX_PRUNE_SEGMENTS: [PruneSegment; 1] = [PruneSegment::TransactionLookup];

/// Resets stage and prune checkpoints for stages whose output is not included
/// in the snapshot inside an existing write transaction.
///
/// A snapshot's mdbx comes from a fully synced node, so it has stage checkpoints
/// at the tip for `TransactionLookup`, `IndexAccountHistory`, and
/// `IndexStorageHistory`. When the download omits the RocksDB indices those
/// stages produced, we reset their checkpoints to block 0. Otherwise the
/// pipeline would see "already done" and skip rebuilding entirely.
///
/// We intentionally do not reset `SenderRecovery`: sender static files are
/// distributed for archive downloads, and non-archive downloads rely on the
/// configured prune checkpoints for this segment.
pub(crate) fn reset_index_stage_checkpoints_tx<Tx>(tx: &Tx) -> eyre::Result<()>
where
    Tx: DbTx + DbTxMut,
{
    for stage_id in INDEX_STAGE_IDS {
        tx.put::<tables::StageCheckpoints>(stage_id.to_string(), StageCheckpoint::default())?;

        // Also clear any stage-specific progress data
        tx.delete::<tables::StageCheckpointProgresses>(stage_id.to_string(), None)?;

        info!(target: "reth::cli", stage = stage_id, "Reset stage checkpoint to block 0");
    }

    // Clear the transaction lookup prune checkpoint so the pruner doesn't inherit
    // state from the source node.
    for segment in INDEX_PRUNE_SEGMENTS {
        tx.delete::<tables::PruneCheckpoints>(segment, None)?;
    }

    Ok(())
}

/// Generates a [`Config`] from per-component range selections.
///
/// When all data components are selected as `All`, no pruning is configured (archive node).
/// Otherwise, `--minimal` style pruning is applied for missing/partial components.
pub(crate) fn config_for_selections(
    selections: &BTreeMap<SnapshotComponentType, ComponentSelection>,
    manifest: &SnapshotManifest,
    preset: Option<SelectionPreset>,
    chain_spec: Option<&impl EthereumHardforks>,
) -> Config {
    let selection_for = |ty| selections.get(&ty).copied().unwrap_or(ComponentSelection::None);

    let tx_sel = selection_for(SnapshotComponentType::Transactions);
    let senders_sel = selection_for(SnapshotComponentType::TransactionSenders);
    let receipt_sel = selection_for(SnapshotComponentType::Receipts);
    let account_cs_sel = selection_for(SnapshotComponentType::AccountChangesets);
    let storage_cs_sel = selection_for(SnapshotComponentType::StorageChangesets);

    // Archive node — all data components present, no pruning
    let is_archive = [tx_sel, senders_sel, receipt_sel, account_cs_sel, storage_cs_sel]
        .iter()
        .all(|s| *s == ComponentSelection::All);

    // Extract blocks_per_file from manifest for all component types
    let blocks_per_file = |ty: SnapshotComponentType| -> Option<u64> {
        match manifest.component(ty)? {
            ComponentManifest::Chunked(c) => Some(c.blocks_per_file),
            ComponentManifest::Single(_) | ComponentManifest::Files(_) => None,
        }
    };
    let static_files = StaticFilesConfig {
        blocks_per_file: BlocksPerFileConfig {
            headers: blocks_per_file(SnapshotComponentType::Headers),
            transactions: blocks_per_file(SnapshotComponentType::Transactions),
            receipts: blocks_per_file(SnapshotComponentType::Receipts),
            transaction_senders: blocks_per_file(SnapshotComponentType::TransactionSenders),
            account_change_sets: blocks_per_file(SnapshotComponentType::AccountChangesets),
            storage_change_sets: blocks_per_file(SnapshotComponentType::StorageChangesets),
        },
    };

    if matches!(preset, Some(SelectionPreset::Archive)) {
        return Config { static_files, ..Default::default() };
    }

    if matches!(preset, Some(SelectionPreset::Minimal)) {
        return Config {
            prune: PruneConfig {
                segments: DefaultPruningValues::get_global().minimal_prune_modes.clone(),
                ..Default::default()
            },
            static_files,
            ..Default::default()
        };
    }

    if matches!(preset, Some(SelectionPreset::Full)) {
        let defaults = DefaultPruningValues::get_global();
        let mut segments = defaults.full_prune_modes.clone();

        if defaults.full_bodies_history_use_pre_merge {
            segments.bodies_history = chain_spec.and_then(|chain_spec| {
                chain_spec
                    .ethereum_fork_activation(EthereumHardfork::Paris)
                    .block_number()
                    .map(PruneMode::Before)
            });
        }

        return Config {
            prune: PruneConfig { segments, ..Default::default() },
            static_files,
            ..Default::default()
        };
    }

    if is_archive {
        return Config { static_files, ..Default::default() };
    }

    let mut config = Config::default();
    let mut prune = PruneConfig::default();

    if senders_sel != ComponentSelection::All {
        prune.segments.sender_recovery = Some(PruneMode::Full);
    }
    prune.segments.transaction_lookup = Some(PruneMode::Full);

    if let Some(mode) = selection_to_prune_mode(tx_sel, Some(MINIMUM_UNWIND_SAFE_DISTANCE)) {
        prune.segments.bodies_history = Some(mode);
    }

    if let Some(mode) = selection_to_prune_mode(receipt_sel, Some(MINIMUM_DISTANCE)) {
        prune.segments.receipts = Some(mode);
    }

    if let Some(mode) = selection_to_prune_mode(account_cs_sel, Some(MINIMUM_UNWIND_SAFE_DISTANCE))
    {
        prune.segments.account_history = Some(mode);
    }

    if let Some(mode) = selection_to_prune_mode(storage_cs_sel, Some(MINIMUM_UNWIND_SAFE_DISTANCE))
    {
        prune.segments.storage_history = Some(mode);
    }

    config.prune = prune;
    config.static_files = static_files;
    config
}

/// Converts a [`ComponentSelection`] to an optional [`PruneMode`].
///
/// `min_distance` enforces the minimum blocks required for this segment.
/// When set, `None` and distances below the minimum are clamped to it
/// instead of producing `PruneMode::Full` which reth would reject.
pub(super) fn selection_to_prune_mode(
    sel: ComponentSelection,
    min_distance: Option<u64>,
) -> Option<PruneMode> {
    match sel {
        ComponentSelection::All => None,
        ComponentSelection::Distance(d) => {
            Some(PruneMode::Distance(min_distance.map_or(d, |min| d.max(min))))
        }
        ComponentSelection::Since(block) => Some(PruneMode::Before(block)),
        ComponentSelection::None => Some(min_distance.map_or(PruneMode::Full, PruneMode::Distance)),
    }
}

/// Human-readable prune config summary.
pub(crate) fn describe_prune_config(config: &Config) -> Vec<String> {
    let segments = &config.prune.segments;

    [
        ("sender_recovery", segments.sender_recovery),
        ("transaction_lookup", segments.transaction_lookup),
        ("bodies_history", segments.bodies_history),
        ("receipts", segments.receipts),
        ("account_history", segments.account_history),
        ("storage_history", segments.storage_history),
    ]
    .into_iter()
    .filter_map(|(name, mode)| mode.map(|m| format!("{name}={}", format_mode(&m))))
    .collect()
}

/// Formats one prune mode for the generated config summary.
fn format_mode(mode: &PruneMode) -> String {
    match mode {
        PruneMode::Full => "\"full\"".to_string(),
        PruneMode::Distance(d) => format!("{{ distance = {d} }}"),
        PruneMode::Before(b) => format!("{{ before = {b} }}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::{tests::set_layout, DownloadCommand};
    use clap::Parser;
    use reth_db::Database;
    use reth_db_api::models::StoredBlockBodyIndices;
    use reth_ethereum_cli::chainspec::EthereumChainSpecParser;

    /// Empty manifest for tests that only care about prune config.
    fn empty_manifest() -> SnapshotManifest {
        SnapshotManifest {
            block: 0,
            chain_id: 1,
            storage_version: 2,
            timestamp: 0,
            base_url: None,
            reth_version: None,
            components: BTreeMap::new(),
            extensions: Default::default(),
        }
    }

    #[test]
    fn write_prune_checkpoints_marks_omitted_segments() {
        let dir = tempfile::tempdir().unwrap();
        let db = reth_db::init_db(dir.path(), reth_db::mdbx::DatabaseArguments::default()).unwrap();

        let mut selections = BTreeMap::new();
        selections.insert(SnapshotComponentType::State, ComponentSelection::All);
        selections.insert(SnapshotComponentType::Headers, ComponentSelection::All);
        selections.insert(SnapshotComponentType::RocksdbIndices, ComponentSelection::All);
        let snapshot_block = 21_000_000;
        let manifest = SnapshotManifest { block: snapshot_block, ..empty_manifest() };
        let config =
            config_for_selections(&selections, &manifest, None, None::<&reth_chainspec::ChainSpec>);

        {
            let tx = db.tx_mut().unwrap();
            write_prune_checkpoints_tx(&tx, &config, &selections, &manifest, true, false).unwrap();
            tx.commit().unwrap();
        }

        // The restored RocksDB index has no pruning progress.
        let tx = db.tx().unwrap();
        assert_eq!(
            tx.get::<tables::PruneCheckpoints>(PruneSegment::TransactionLookup).unwrap(),
            None
        );

        // The other components were omitted.
        for segment in [
            PruneSegment::SenderRecovery,
            PruneSegment::Receipts,
            PruneSegment::AccountHistory,
            PruneSegment::StorageHistory,
            PruneSegment::Bodies,
        ] {
            let checkpoint = tx
                .get::<tables::PruneCheckpoints>(segment)
                .unwrap()
                .unwrap_or_else(|| panic!("expected checkpoint for {segment}"));
            assert_eq!(checkpoint.block_number, Some(snapshot_block));
            // No BlockBodyIndices in empty DB, so tx_number should be None
            assert_eq!(checkpoint.tx_number, None);
        }
    }

    #[test]
    fn transaction_checkpoints_keep_source_progress_and_download_bounds() -> eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let manifest = SnapshotManifest { block: 10, ..empty_manifest() };
        let db = reth_db::init_db(dir.path(), mdbx::DatabaseArguments::test())?;
        let tx = db.tx_mut()?;
        let mut config = Config::default();
        config.prune.segments.bodies_history = Some(PruneMode::Distance(20_000));
        let source = PruneCheckpoint {
            block_number: Some(2),
            tx_number: Some(31),
            prune_mode: PruneMode::Full,
        };
        tx.put::<tables::PruneCheckpoints>(PruneSegment::Bodies, source)?;
        tx.put::<tables::BlockBodyIndices>(
            5,
            StoredBlockBodyIndices { first_tx_num: 60, tx_count: 3 },
        )?;
        tx.put::<tables::BlockBodyIndices>(
            10,
            StoredBlockBodyIndices { first_tx_num: 100, tx_count: 2 },
        )?;
        let selections = BTreeMap::from([
            (SnapshotComponentType::Transactions, ComponentSelection::All),
            (SnapshotComponentType::Receipts, ComponentSelection::Since(6)),
            (SnapshotComponentType::RocksdbIndices, ComponentSelection::All),
        ]);
        write_prune_checkpoints_tx(&tx, &config, &selections, &manifest, true, false)?;
        assert_eq!(
            tx.get::<tables::PruneCheckpoints>(PruneSegment::Bodies)?,
            Some(PruneCheckpoint { prune_mode: PruneMode::Distance(20_000), ..source })
        );
        for (segment, boundary, tx_number) in
            [(PruneSegment::Receipts, 5, 62), (PruneSegment::SenderRecovery, 10, 101)]
        {
            assert_eq!(
                tx.get::<tables::PruneCheckpoints>(segment)?,
                Some(PruneCheckpoint {
                    tx_number: Some(tx_number),
                    ..PruneCheckpoint::pruned_through(boundary)
                }),
            );
        }
        assert_eq!(tx.get::<tables::PruneCheckpoints>(PruneSegment::TransactionLookup)?, None);
        tx.delete::<tables::PruneCheckpoints>(PruneSegment::Bodies, None)?;
        write_prune_checkpoints_tx(&tx, &config, &selections, &manifest, true, false)?;
        assert_eq!(
            tx.get::<tables::PruneCheckpoints>(PruneSegment::Bodies)?,
            Some(PruneCheckpoint {
                block_number: None,
                tx_number: None,
                prune_mode: PruneMode::Distance(20_000)
            })
        );
        Ok(())
    }

    #[test]
    fn bundled_receipts_require_unbroken_transaction_coverage() -> eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let db = reth_db::init_db(dir.path(), mdbx::DatabaseArguments::test())?;
        let tx = db.tx_mut()?;
        let manifest = SnapshotManifest { block: 2, ..empty_manifest() };
        tx.put::<tables::BlockBodyIndices>(
            0,
            StoredBlockBodyIndices { first_tx_num: 0, tx_count: 1 },
        )?;
        tx.put::<tables::BlockBodyIndices>(
            2,
            StoredBlockBodyIndices { first_tx_num: 3, tx_count: 1 },
        )?;
        let source = PruneCheckpoint { tx_number: Some(0), ..PruneCheckpoint::pruned_through(0) };
        for number in [1, 2, 3] {
            tx.put::<tables::Receipts>(number, Default::default())?;
        }
        let mut config = Config::default();
        config.prune.segments.receipts = Some(PruneMode::Distance(500));
        for layout in [false, true] {
            tx.put::<tables::PruneCheckpoints>(PruneSegment::Receipts, source)?;
            write_prune_checkpoints_tx(&tx, &config, &BTreeMap::new(), &manifest, layout, false)?;
            assert_eq!(
                tx.get::<tables::PruneCheckpoints>(PruneSegment::Receipts)?,
                Some(PruneCheckpoint { prune_mode: PruneMode::Distance(500), ..source })
            );
        }
        tx.delete::<tables::Receipts>(2, None)?;
        write_prune_checkpoints_tx(&tx, &config, &BTreeMap::new(), &manifest, true, false)?;
        assert_eq!(
            tx.get::<tables::PruneCheckpoints>(PruneSegment::Receipts)?,
            Some(PruneCheckpoint {
                block_number: Some(2),
                tx_number: Some(3),
                prune_mode: PruneMode::Distance(500)
            })
        );
        Ok(())
    }

    /// Writes prune checkpoints for state history retained over `distance` blocks.
    fn history_checkpoint(snapshot_block: u64, distance: u64) -> Option<PruneCheckpoint> {
        let dir = tempfile::tempdir().unwrap();
        let manifest = SnapshotManifest { block: snapshot_block, ..empty_manifest() };
        let db = reth_db::init_db(dir.path(), reth_db::mdbx::DatabaseArguments::default()).unwrap();
        let selections = BTreeMap::from([(
            SnapshotComponentType::AccountChangesets,
            ComponentSelection::Distance(distance),
        )]);
        let config =
            config_for_selections(&selections, &manifest, None, None::<&reth_chainspec::ChainSpec>);

        let tx = db.tx_mut().unwrap();
        write_prune_checkpoints_tx(&tx, &config, &selections, &manifest, true, false).unwrap();
        tx.commit().unwrap();

        db.tx().unwrap().get::<tables::PruneCheckpoints>(PruneSegment::AccountHistory).unwrap()
    }

    #[test]
    fn write_prune_checkpoints_sets_history_to_retained_window_start() {
        let checkpoint = history_checkpoint(21_000_000, 500_000).unwrap();
        assert_eq!(checkpoint.block_number, Some(20_500_000));
        assert_eq!(checkpoint.prune_mode, PruneMode::Distance(500_000));
    }

    #[test]
    fn write_prune_checkpoints_skips_history_shorter_than_distance() {
        assert_eq!(history_checkpoint(100_000, 500_000), None);
    }

    #[test]
    fn write_prune_checkpoints_archive_no_checkpoints() -> eyre::Result<()> {
        let command = DownloadCommand::<EthereumChainSpecParser>::parse_from(["reth", "--archive"]);
        let included: BTreeMap<_, _> =
            SnapshotComponentType::ALL.iter().map(|&ty| (ty, ComponentSelection::All)).collect();
        let mut omitted = included.clone();
        omitted.remove(&SnapshotComponentType::RocksdbIndices);
        let indexed = StageCheckpoint::new(10);
        let stage = INDEX_STAGE_IDS[0].to_owned();
        for (layout, selections, expected) in [
            (None, &omitted, indexed),
            (Some(serde_json::json!(StorageSettings::v2())), &omitted, StageCheckpoint::default()),
            (Some(serde_json::json!(StorageSettings::v2())), &included, indexed),
            (Some(serde_json::json!({"receipts_in_static_files": true})), &included, indexed),
        ] {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("db");
            {
                let db = reth_db::init_db(&path, reth_db::mdbx::DatabaseArguments::test())?;
                let tx = db.tx_mut()?;
                if let Some(settings) = layout {
                    set_layout(&tx, settings);
                }
                tx.put::<tables::StageCheckpoints>(stage.clone(), indexed)?;
                tx.commit()?;
            }
            command.finalize_modular_download(
                selections,
                &empty_manifest(),
                command.download_config(
                    selections,
                    &empty_manifest(),
                    Some(SelectionPreset::Archive),
                ),
                dir.path(),
            )?;
            let db = reth_db::init_db(&path, reth_db::mdbx::DatabaseArguments::test())?;
            let tx = db.tx()?;
            assert_eq!(tx.get::<tables::StageCheckpoints>(stage.clone())?, Some(expected));
            for segment in [PruneSegment::SenderRecovery, PruneSegment::TransactionLookup] {
                assert_eq!(tx.get::<tables::PruneCheckpoints>(segment)?, None);
            }
        }
        Ok(())
    }

    #[test]
    fn selections_all_no_pruning() {
        let mut selections = BTreeMap::new();
        for ty in SnapshotComponentType::ALL {
            selections.insert(ty, ComponentSelection::All);
        }
        let config = config_for_selections(
            &selections,
            &empty_manifest(),
            None,
            None::<&reth_chainspec::ChainSpec>,
        );
        // Archive node — nothing pruned
        assert_eq!(config.prune.segments.transaction_lookup, None);
        assert_eq!(config.prune.segments.sender_recovery, None);
        assert_eq!(config.prune.segments.bodies_history, None);
        assert_eq!(config.prune.segments.receipts, None);
        assert_eq!(config.prune.segments.account_history, None);
        assert_eq!(config.prune.segments.storage_history, None);
    }

    #[test]
    fn selections_none_clamps_to_minimum_distance() {
        let mut selections = BTreeMap::new();
        selections.insert(SnapshotComponentType::State, ComponentSelection::All);
        selections.insert(SnapshotComponentType::Headers, ComponentSelection::All);
        let config = config_for_selections(
            &selections,
            &empty_manifest(),
            None,
            None::<&reth_chainspec::ChainSpec>,
        );
        assert_eq!(config.prune.segments.transaction_lookup, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.sender_recovery, Some(PruneMode::Full));
        // All segments clamped to their minimum distances
        assert_eq!(
            config.prune.segments.bodies_history,
            Some(PruneMode::Distance(MINIMUM_UNWIND_SAFE_DISTANCE))
        );
        assert_eq!(config.prune.segments.receipts, Some(PruneMode::Distance(MINIMUM_DISTANCE)));
        assert_eq!(
            config.prune.segments.account_history,
            Some(PruneMode::Distance(MINIMUM_UNWIND_SAFE_DISTANCE))
        );
        assert_eq!(
            config.prune.segments.storage_history,
            Some(PruneMode::Distance(MINIMUM_UNWIND_SAFE_DISTANCE))
        );
    }

    #[test]
    fn selections_distance_maps_bodies_history() {
        let mut selections = BTreeMap::new();
        selections.insert(SnapshotComponentType::State, ComponentSelection::All);
        selections.insert(SnapshotComponentType::Headers, ComponentSelection::All);
        selections
            .insert(SnapshotComponentType::Transactions, ComponentSelection::Distance(10_064));
        selections.insert(SnapshotComponentType::Receipts, ComponentSelection::None);
        selections
            .insert(SnapshotComponentType::AccountChangesets, ComponentSelection::Distance(10_064));
        selections
            .insert(SnapshotComponentType::StorageChangesets, ComponentSelection::Distance(10_064));
        let config = config_for_selections(
            &selections,
            &empty_manifest(),
            None,
            None::<&reth_chainspec::ChainSpec>,
        );

        assert_eq!(config.prune.segments.transaction_lookup, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.sender_recovery, Some(PruneMode::Full));
        // Bodies follows tx selection
        assert_eq!(config.prune.segments.bodies_history, Some(PruneMode::Distance(10_064)));
        assert_eq!(config.prune.segments.receipts, Some(PruneMode::Distance(MINIMUM_DISTANCE)));
        assert_eq!(config.prune.segments.account_history, Some(PruneMode::Distance(10_064)));
        assert_eq!(config.prune.segments.storage_history, Some(PruneMode::Distance(10_064)));
    }

    #[test]
    fn selections_since_maps_to_before_prune_mode() {
        let mut selections = BTreeMap::new();
        selections.insert(SnapshotComponentType::State, ComponentSelection::All);
        selections.insert(SnapshotComponentType::Headers, ComponentSelection::All);
        selections
            .insert(SnapshotComponentType::Transactions, ComponentSelection::Since(15_537_394));
        selections.insert(SnapshotComponentType::Receipts, ComponentSelection::Since(15_537_394));
        selections.insert(
            SnapshotComponentType::AccountChangesets,
            ComponentSelection::Since(15_537_394),
        );
        selections.insert(
            SnapshotComponentType::StorageChangesets,
            ComponentSelection::Since(15_537_394),
        );

        let config = config_for_selections(
            &selections,
            &empty_manifest(),
            None,
            None::<&reth_chainspec::ChainSpec>,
        );

        assert_eq!(config.prune.segments.bodies_history, Some(PruneMode::Before(15_537_394)));
        assert_eq!(config.prune.segments.receipts, Some(PruneMode::Before(15_537_394)));
        assert_eq!(config.prune.segments.account_history, Some(PruneMode::Before(15_537_394)));
        assert_eq!(config.prune.segments.storage_history, Some(PruneMode::Before(15_537_394)));
    }

    #[test]
    fn full_preset_matches_default_full_prune_config() {
        let mut selections = BTreeMap::new();
        selections.insert(SnapshotComponentType::State, ComponentSelection::All);
        selections.insert(SnapshotComponentType::Headers, ComponentSelection::All);
        selections
            .insert(SnapshotComponentType::Transactions, ComponentSelection::Distance(500_000));
        selections.insert(SnapshotComponentType::Receipts, ComponentSelection::Distance(10_064));

        let chain_spec = reth_chainspec::MAINNET.clone();
        let config = config_for_selections(
            &selections,
            &empty_manifest(),
            Some(SelectionPreset::Full),
            Some(chain_spec.as_ref()),
        );

        assert_eq!(config.prune.segments.sender_recovery, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.transaction_lookup, None);
        assert_eq!(
            config.prune.segments.receipts,
            Some(PruneMode::Distance(MINIMUM_UNWIND_SAFE_DISTANCE))
        );
        assert_eq!(
            config.prune.segments.account_history,
            Some(PruneMode::Distance(MINIMUM_UNWIND_SAFE_DISTANCE))
        );
        assert_eq!(
            config.prune.segments.storage_history,
            Some(PruneMode::Distance(MINIMUM_UNWIND_SAFE_DISTANCE))
        );

        let paris_block = chain_spec
            .ethereum_fork_activation(EthereumHardfork::Paris)
            .block_number()
            .expect("mainnet Paris block should be known");
        assert_eq!(config.prune.segments.bodies_history, Some(PruneMode::Before(paris_block)));
    }

    #[test]
    fn minimal_preset_matches_default_minimal_prune_config() {
        let config = config_for_selections(
            &BTreeMap::from([(SnapshotComponentType::AccountChangesets, ComponentSelection::All)]),
            &empty_manifest(),
            Some(SelectionPreset::Minimal),
            None::<&reth_chainspec::ChainSpec>,
        );

        assert_eq!(&config.prune.segments, &DefaultPruningValues::get_global().minimal_prune_modes);
    }

    #[test]
    fn describe_selections_all_no_pruning() {
        let mut selections = BTreeMap::new();
        for ty in SnapshotComponentType::ALL {
            selections.insert(ty, ComponentSelection::All);
        }
        let config = config_for_selections(
            &selections,
            &empty_manifest(),
            None,
            None::<&reth_chainspec::ChainSpec>,
        );
        let desc = describe_prune_config(&config);
        // Archive node — no prune segments described
        assert!(desc.is_empty());
    }

    #[test]
    fn describe_selections_with_distances() {
        let mut selections = BTreeMap::new();
        selections.insert(SnapshotComponentType::State, ComponentSelection::All);
        selections.insert(SnapshotComponentType::Headers, ComponentSelection::All);
        selections
            .insert(SnapshotComponentType::Transactions, ComponentSelection::Distance(10_064));
        selections.insert(SnapshotComponentType::Receipts, ComponentSelection::None);
        let config = config_for_selections(
            &selections,
            &empty_manifest(),
            None,
            None::<&reth_chainspec::ChainSpec>,
        );
        let desc = describe_prune_config(&config);
        assert!(desc.iter().any(|d| d == "sender_recovery=\"full\""));
        // Bodies follows tx selection
        assert!(desc.iter().any(|d| d == "bodies_history={ distance = 10064 }"));
        assert!(desc.iter().any(|d| d == "receipts={ distance = 64 }"));
    }

    #[test]
    fn reset_index_stage_checkpoints_clears_only_rocksdb_index_stages() {
        let dir = tempfile::tempdir().unwrap();
        let db = reth_db::init_db(dir.path(), reth_db::mdbx::DatabaseArguments::default()).unwrap();

        // Simulate a fully synced node: set stage checkpoints at tip
        let tip_checkpoint = StageCheckpoint::new(24_500_000);
        {
            let tx = db.tx_mut().unwrap();
            for stage_id in INDEX_STAGE_IDS {
                tx.put::<tables::StageCheckpoints>(stage_id.to_string(), tip_checkpoint).unwrap();
            }
            for segment in INDEX_PRUNE_SEGMENTS {
                tx.put::<tables::PruneCheckpoints>(
                    segment,
                    PruneCheckpoint {
                        block_number: Some(24_500_000),
                        tx_number: None,
                        prune_mode: PruneMode::Full,
                    },
                )
                .unwrap();
            }

            // Sender recovery and history checkpoints should be preserved by reset.
            tx.put::<tables::StageCheckpoints>("SenderRecovery".to_string(), tip_checkpoint)
                .unwrap();
            for segment in [
                PruneSegment::SenderRecovery,
                PruneSegment::AccountHistory,
                PruneSegment::StorageHistory,
            ] {
                tx.put::<tables::PruneCheckpoints>(
                    segment,
                    PruneCheckpoint {
                        block_number: Some(24_500_000),
                        tx_number: None,
                        prune_mode: PruneMode::Full,
                    },
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }

        // Reset
        {
            let tx = db.tx_mut().unwrap();
            reset_index_stage_checkpoints_tx(&tx).unwrap();
            tx.commit().unwrap();
        }

        // Verify stage checkpoints are at block 0
        let tx = db.tx().unwrap();
        for stage_id in INDEX_STAGE_IDS {
            let checkpoint = tx
                .get::<tables::StageCheckpoints>(stage_id.to_string())
                .unwrap()
                .expect("checkpoint should exist");
            assert_eq!(checkpoint.block_number, 0, "stage {stage_id} should be reset to block 0");
        }

        // Verify prune checkpoints are deleted
        for segment in INDEX_PRUNE_SEGMENTS {
            assert!(
                tx.get::<tables::PruneCheckpoints>(segment).unwrap().is_none(),
                "prune checkpoint for {segment} should be deleted"
            );
        }

        // Verify sender checkpoints are left untouched.
        let sender_stage_checkpoint = tx
            .get::<tables::StageCheckpoints>("SenderRecovery".to_string())
            .unwrap()
            .expect("sender checkpoint should exist");
        assert_eq!(sender_stage_checkpoint.block_number, tip_checkpoint.block_number);

        let sender_prune_checkpoint = tx
            .get::<tables::PruneCheckpoints>(PruneSegment::SenderRecovery)
            .unwrap()
            .expect("sender prune checkpoint should exist");
        assert_eq!(sender_prune_checkpoint.block_number, Some(tip_checkpoint.block_number));

        // History checkpoints record where changesets start, so they must survive the reset.
        for segment in [PruneSegment::AccountHistory, PruneSegment::StorageHistory] {
            let checkpoint = tx
                .get::<tables::PruneCheckpoints>(segment)
                .unwrap()
                .expect("history prune checkpoint should exist");
            assert_eq!(checkpoint.block_number, Some(tip_checkpoint.block_number));
        }
    }

    #[test]
    fn history_checkpoints_survive_index_reset_without_rocksdb_indices() {
        let snapshot_block = 21_000_000;
        let manifest = SnapshotManifest { block: snapshot_block, ..empty_manifest() };
        let changesets_start = 20_000_000;
        let dir = tempfile::tempdir().unwrap();
        let db = reth_db::init_db(dir.path(), reth_db::mdbx::DatabaseArguments::default()).unwrap();
        let selections = BTreeMap::from([
            (SnapshotComponentType::State, ComponentSelection::All),
            (SnapshotComponentType::Headers, ComponentSelection::All),
            (SnapshotComponentType::AccountChangesets, ComponentSelection::Since(changesets_start)),
            (SnapshotComponentType::StorageChangesets, ComponentSelection::Since(changesets_start)),
        ]);
        let config =
            config_for_selections(&selections, &manifest, None, None::<&reth_chainspec::ChainSpec>);
        // Same order as the download finalization.
        let tx = db.tx_mut().unwrap();
        write_prune_checkpoints_tx(&tx, &config, &selections, &manifest, true, false).unwrap();
        reset_index_stage_checkpoints_tx(&tx).unwrap();

        for segment in [PruneSegment::AccountHistory, PruneSegment::StorageHistory] {
            let checkpoint = tx.get::<tables::PruneCheckpoints>(segment).unwrap();
            assert_eq!(
                checkpoint.and_then(|checkpoint| checkpoint.block_number),
                Some(changesets_start - 1),
                "{segment} checkpoint should mark where the downloaded changesets start"
            );
        }
    }

    #[test]
    fn history_checkpoints_do_not_advertise_undownloaded_changesets() {
        let snapshot_block = 21_000_000;
        let manifest = SnapshotManifest { block: snapshot_block, ..empty_manifest() };
        for (selection, expected) in [
            (None, Some(snapshot_block)),
            (Some(ComponentSelection::None), Some(snapshot_block)),
            (Some(ComponentSelection::Distance(1)), Some(snapshot_block - 1)),
            (Some(ComponentSelection::Since(snapshot_block)), Some(snapshot_block - 1)),
            (Some(ComponentSelection::Since(0)), None),
            (Some(ComponentSelection::All), None),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let db =
                reth_db::init_db(dir.path(), reth_db::mdbx::DatabaseArguments::default()).unwrap();
            let mut selections = BTreeMap::from([
                (SnapshotComponentType::State, ComponentSelection::All),
                (SnapshotComponentType::Headers, ComponentSelection::All),
                (SnapshotComponentType::RocksdbIndices, ComponentSelection::All),
            ]);
            if let Some(selection) = selection {
                selections.insert(SnapshotComponentType::AccountChangesets, selection);
                selections.insert(SnapshotComponentType::StorageChangesets, selection);
            }
            let config = config_for_selections(
                &selections,
                &manifest,
                None,
                None::<&reth_chainspec::ChainSpec>,
            );
            let tx = db.tx_mut().unwrap();
            write_prune_checkpoints_tx(&tx, &config, &selections, &manifest, true, false).unwrap();
            let mut checkpoints = Vec::new();
            for segment in [PruneSegment::AccountHistory, PruneSegment::StorageHistory] {
                let checkpoint = tx.get::<tables::PruneCheckpoints>(segment).unwrap();
                assert_eq!(checkpoint.and_then(|checkpoint| checkpoint.block_number), expected);
                if let Some(checkpoint) = checkpoint {
                    checkpoints.push((segment, checkpoint));
                }
            }
            if let Some(boundary) = expected {
                assert!(config
                    .prune
                    .segments
                    .ensure_unwind_target_unpruned(snapshot_block, boundary - 1, &checkpoints,)
                    .is_err());
                assert!(config
                    .prune
                    .segments
                    .ensure_unwind_target_unpruned(snapshot_block, boundary, &checkpoints,)
                    .is_ok());
            }
        }
    }

    #[test]
    fn history_checkpoints_preserve_source_pruning_boundary() {
        let snapshot_block = 21_000_000;
        let manifest = SnapshotManifest { block: snapshot_block, ..empty_manifest() };
        let selected_boundary = 20_500_000;
        for selection in [
            ComponentSelection::Distance(snapshot_block - selected_boundary),
            ComponentSelection::Since(selected_boundary + 1),
        ] {
            for existing_boundary in [None, Some(20_000_000), Some(20_750_000)] {
                let dir = tempfile::tempdir().unwrap();
                let db = reth_db::init_db(dir.path(), reth_db::mdbx::DatabaseArguments::default())
                    .unwrap();
                let selections = BTreeMap::from([
                    (SnapshotComponentType::AccountChangesets, selection),
                    (SnapshotComponentType::StorageChangesets, selection),
                    (SnapshotComponentType::RocksdbIndices, ComponentSelection::All),
                ]);
                let config = config_for_selections(
                    &selections,
                    &manifest,
                    None,
                    None::<&reth_chainspec::ChainSpec>,
                );
                let tx = db.tx_mut().unwrap();
                for segment in [PruneSegment::AccountHistory, PruneSegment::StorageHistory] {
                    tx.put::<tables::PruneCheckpoints>(
                        segment,
                        PruneCheckpoint {
                            block_number: existing_boundary,
                            tx_number: Some(100),
                            prune_mode: PruneMode::Full,
                        },
                    )
                    .unwrap();
                }

                write_prune_checkpoints_tx(&tx, &config, &selections, &manifest, true, false)
                    .unwrap();

                let expected = existing_boundary.unwrap_or_default().max(selected_boundary);
                for (segment, mode) in [
                    (PruneSegment::AccountHistory, config.prune.segments.account_history),
                    (PruneSegment::StorageHistory, config.prune.segments.storage_history),
                ] {
                    assert_eq!(
                        tx.get::<tables::PruneCheckpoints>(segment).unwrap(),
                        Some(PruneCheckpoint {
                            block_number: Some(expected),
                            tx_number: None,
                            prune_mode: mode.unwrap(),
                        }),
                    );
                }
            }
        }
    }

    #[test]
    fn snapshot_without_metadata_table_uses_v1() -> eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let db = reth_db::create_db(dir.path(), mdbx::DatabaseArguments::test())?;
        assert!(!snapshot_uses_v2(&db.tx()?, &BTreeMap::new())?);
        Ok(())
    }

    #[test]
    fn rejects_unsupported_snapshot_layouts() -> eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let db = reth_db::init_db(dir.path(), reth_db::mdbx::DatabaseArguments::test())?;
        let tx = db.tx_mut()?;
        let selections =
            BTreeMap::from([(SnapshotComponentType::TransactionSenders, ComponentSelection::All)]);
        for bytes in [b"invalid".as_slice(), br#"{}"#, br#"{"future_flag":false}"#] {
            tx.put::<tables::Metadata>(keys::STORAGE_SETTINGS.to_owned(), bytes.to_vec())?;
            assert!(!snapshot_uses_v2(&tx, &selections)?);
        }
        for (settings, expected) in [
            (serde_json::json!({"account_changesets_in_static_files": true, "unknown": [1]}),
                "Unsupported legacy snapshot setting for this download: account_changesets_in_static_files=true"),
            (serde_json::json!({"receipts_in_static_files": false, "account_changesets_in_static_files": true}),
                "Unsupported legacy snapshot setting for this download: account_changesets_in_static_files=true"),
            (serde_json::json!({"receipts_in_static_files": false, "account_history_in_rocksdb": true}),
                "Unsupported legacy snapshot setting for this download: account_history_in_rocksdb=true"),
            (serde_json::json!({"receipts_in_static_files": false, "transaction_senders_in_static_files": true}),
                "Unsupported legacy snapshot setting for this download: transaction_senders_in_static_files=true"),
        ] {
            set_layout(&tx, settings);
            assert_eq!(snapshot_uses_v2(&tx, &selections).unwrap_err().to_string(), expected);
        }
        Ok(())
    }
}
