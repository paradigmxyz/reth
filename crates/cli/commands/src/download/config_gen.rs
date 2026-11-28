use crate::download::{
    manifest::{ComponentManifest, ComponentSelection, SnapshotComponentType, SnapshotManifest},
    SelectionPreset,
};
use reth_chainspec::{EthereumHardfork, EthereumHardforks};
use reth_config::config::{BlocksPerFileConfig, Config, PruneConfig, StaticFilesConfig};
use reth_db::{mdbx, models::StorageSettings, tables, DatabaseError};
use reth_db_api::transaction::{DbTx, DbTxMut};
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
    write_config_to_path(config, &data_dir.join("reth.toml"))
}

/// Writes the generated config only if the selected node config does not exist.
pub(super) fn write_config_to_path(config: &Config, config_path: &Path) -> eyre::Result<bool> {
    if config_path.try_exists()? {
        info!(target: "reth::cli",
            path = ?config_path,
            "Config file already exists, skipping config generation"
        );
        return Ok(false);
    }

    let toml_str = toml::to_string_pretty(config)?;
    if let Some(parent) = config_path.parent() {
        reth_fs_util::create_dir_all(parent)?;
    }
    reth_fs_util::write(config_path, toml_str)?;

    info!(target: "reth::cli",
        path = ?config_path,
        "Generated config based on downloaded components"
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
    bodies_boundary: Option<u64>,
    source_bodies: Option<PruneCheckpoint>,
) -> eyre::Result<()>
where
    Tx: DbTx + DbTxMut,
{
    let segments = &config.prune.segments;

    let tip_indices = tx.get::<tables::BlockBodyIndices>(manifest.block)?;
    for (segment, mode) in [
        (PruneSegment::SenderRecovery, segments.sender_recovery),
        (PruneSegment::TransactionLookup, segments.transaction_lookup),
        (PruneSegment::Receipts, segments.receipts),
        (PruneSegment::AccountHistory, segments.account_history),
        (PruneSegment::StorageHistory, segments.storage_history),
        (PruneSegment::Bodies, segments.bodies_history),
    ] {
        let restored_checkpoint = |block_number: Option<u64>, tx_number| {
            // Unlimited retention still records the earliest available history.
            let prune_mode = mode.unwrap_or_else(|| {
                PruneMode::Before(block_number.map_or(0, |block| block.saturating_add(1)))
            });
            PruneCheckpoint { block_number, tx_number, prune_mode }
        };
        let checkpoint = match segment {
            // V1 bundles the changesets and indices in MDBX. Their original checkpoints
            // also identify interrupted pruning when the node resumes.
            PruneSegment::AccountHistory |
            PruneSegment::StorageHistory |
            PruneSegment::TransactionLookup
                if !is_v2 =>
            {
                continue
            }
            PruneSegment::AccountHistory | PruneSegment::StorageHistory => {
                let component = if segment == PruneSegment::AccountHistory {
                    SnapshotComponentType::AccountChangesets
                } else {
                    SnapshotComponentType::StorageChangesets
                };
                let target = if manifest.component(component).is_some() &&
                    manifest.chunks_for_distance(component, Some(1)) == 0
                {
                    // A lagging component has no archive covering the snapshot tip.
                    Some(manifest.block)
                } else {
                    match selections.get(&component) {
                        Some(ComponentSelection::All) => None,
                        Some(ComponentSelection::Distance(distance)) => {
                            manifest.block.checked_sub(*distance)
                        }
                        Some(ComponentSelection::Since(block)) => {
                            block.checked_sub(1).map(|n| n.min(manifest.block))
                        }
                        Some(ComponentSelection::None) | None => Some(manifest.block),
                    }
                };
                let source = tx.get::<tables::PruneCheckpoints>(segment)?;
                let (block_number, tx_number) = match (target, source) {
                    (Some(target), source) => {
                        (Some(target).max(source.and_then(|cp| cp.block_number)), None)
                    }
                    (None, Some(source)) => (source.block_number, source.tx_number),
                    (None, None) => continue,
                };
                restored_checkpoint(block_number, tx_number)
            }
            PruneSegment::Bodies => {
                let source_block = source_bodies.and_then(|checkpoint| checkpoint.block_number);
                let block_number = bodies_boundary.max(source_block);
                if block_number.is_none() && mode.is_none() && source_bodies.is_none() {
                    continue
                }
                let mut tx_number = source_bodies.and_then(|checkpoint| checkpoint.tx_number);
                if let Some(block) = bodies_boundary &&
                    Some(block) > source_block
                {
                    let indices = if block == manifest.block {
                        tip_indices
                    } else {
                        tx.get::<tables::BlockBodyIndices>(block)?
                    };
                    tx_number =
                        tx_number.max(indices.and_then(|body| body.next_tx_num().checked_sub(1)));
                }
                restored_checkpoint(block_number, tx_number)
            }
            _ => {
                let Some(prune_mode) = mode else { continue };
                PruneCheckpoint {
                    block_number: Some(manifest.block),
                    tx_number: tip_indices.map(|body| body.last_tx_num()),
                    prune_mode,
                }
            }
        };
        tx.put::<tables::PruneCheckpoints>(segment, checkpoint)?;

        info!(target: "reth::cli",
            segment = %segment,
            block = ?checkpoint.block_number,
            tx = ?checkpoint.tx_number,
            mode = ?checkpoint.prune_mode,
            "Set prune checkpoint"
        );
    }

    tx.put::<tables::Metadata>(keys::VERIFIED_SNAPSHOT_BODIES.to_owned(), Vec::new())?;
    Ok(())
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
    use crate::download::tests::{
        empty_manifest, history_checkpoints, set_history_checkpoints, set_layout,
    };
    use reth_db::{models::StoredBlockBodyIndices, test_utils::create_test_rw_db, Database};
    use reth_prune_types::UnwindTargetPrunedError;

    #[test]
    fn write_prune_checkpoints_sets_all_segments() -> eyre::Result<()> {
        let db = create_test_rw_db();
        let tx = db.tx_mut()?;
        let tip = 21_000_000;
        let manifest = SnapshotManifest { block: tip, ..empty_manifest() };
        let selections = BTreeMap::new();
        let config =
            config_for_selections(&selections, &manifest, None, None::<&reth_chainspec::ChainSpec>);
        write_prune_checkpoints_tx(&tx, &config, &selections, &manifest, true, Some(tip), None)?;
        for segment in [
            PruneSegment::SenderRecovery,
            PruneSegment::TransactionLookup,
            PruneSegment::Receipts,
            PruneSegment::AccountHistory,
            PruneSegment::StorageHistory,
            PruneSegment::Bodies,
        ] {
            let checkpoint = tx
                .get::<tables::PruneCheckpoints>(segment)?
                .unwrap_or_else(|| panic!("missing checkpoint for {segment}"));
            assert_eq!(checkpoint.block_number, Some(manifest.block), "{segment}");
            assert_eq!(checkpoint.tx_number, None, "{segment}");
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
    fn history_checkpoints_do_not_advertise_undownloaded_changesets() -> eyre::Result<()> {
        let tip = 21_000_000;
        let manifest = SnapshotManifest { block: tip, ..empty_manifest() };
        let db = create_test_rw_db();
        for (selection, expected) in [
            (ComponentSelection::None, Some(tip)),
            (ComponentSelection::Distance(1), Some(tip - 1)),
            (ComponentSelection::Distance(tip + 1), None),
            (ComponentSelection::Since(tip), Some(tip - 1)),
            (ComponentSelection::Since(0), None),
            (ComponentSelection::All, None),
        ] {
            let selections = BTreeMap::from([
                (SnapshotComponentType::AccountChangesets, selection),
                (SnapshotComponentType::StorageChangesets, selection),
            ]);
            let config = config_for_selections(
                &selections,
                &manifest,
                None,
                None::<&reth_chainspec::ChainSpec>,
            );
            let tx = db.tx_mut()?;
            write_prune_checkpoints_tx(
                &tx,
                &config,
                &selections,
                &manifest,
                true,
                Some(tip),
                None,
            )?;
            for segment in [PruneSegment::AccountHistory, PruneSegment::StorageHistory] {
                let checkpoint = tx.get::<tables::PruneCheckpoints>(segment)?;
                assert_eq!(checkpoint.map(|cp| cp.block_number), expected.map(Some), "{segment}");
                if let (Some(boundary), Some(cp)) = (expected, checkpoint) {
                    let modes = &config.prune.segments;
                    let unwind =
                        |block| modes.ensure_unwind_target_unpruned(tip, block, &[(segment, cp)]);
                    let Err(UnwindTargetPrunedError::TargetBelowPrunedHistory {
                        target_block,
                        pruned_block,
                        ..
                    }) = unwind(boundary - 1)
                    else {
                        panic!("expected {segment} to be pruned through {boundary}");
                    };
                    assert_eq!((target_block, pruned_block), (boundary - 1, boundary));
                    unwind(boundary)?;
                }
            }
        }
        Ok(())
    }

    #[test]
    fn history_checkpoints_preserve_source_pruning_boundary() -> eyre::Result<()> {
        let snapshot_block = 21_000_000;
        let manifest = SnapshotManifest { block: snapshot_block, ..empty_manifest() };
        let selected_boundary = 20_500_000;
        let distance = snapshot_block - selected_boundary;
        let since = selected_boundary + 1;
        let selections = BTreeMap::from([
            (SnapshotComponentType::AccountChangesets, ComponentSelection::Distance(distance)),
            (SnapshotComponentType::StorageChangesets, ComponentSelection::Since(since)),
        ]);
        let config =
            config_for_selections(&selections, &manifest, None, None::<&reth_chainspec::ChainSpec>);
        let db = create_test_rw_db();
        for (existing_boundary, expected) in [
            (None, selected_boundary),
            (Some(20_000_000), selected_boundary),
            (Some(20_750_000), 20_750_000),
        ] {
            let tx = db.tx_mut()?;
            let source = PruneCheckpoint {
                block_number: existing_boundary,
                tx_number: Some(100),
                prune_mode: PruneMode::Full,
            };
            set_history_checkpoints(&tx, source, source);
            write_prune_checkpoints_tx(&tx, &config, &selections, &manifest, true, None, None)?;
            let boundary = PruneCheckpoint::pruned_through(expected);
            assert_eq!(
                history_checkpoints(&tx)?,
                [PruneMode::Distance(distance), PruneMode::Before(since)]
                    .map(|prune_mode| Some(PruneCheckpoint { prune_mode, ..boundary }))
            );
        }
        Ok(())
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
        let db = create_test_rw_db();
        let tx = db.tx_mut()?;
        let selections =
            BTreeMap::from([(SnapshotComponentType::TransactionSenders, ComponentSelection::All)]);
        for bytes in [
            b"invalid".as_slice(),
            br#"{}"#,
            br#"{"future_flag":true,"account_history_in_rocksdb":false}"#,
        ] {
            tx.put::<tables::Metadata>(keys::STORAGE_SETTINGS.to_owned(), bytes.to_vec())?;
            assert!(!snapshot_uses_v2(&tx, &selections)?);
        }
        for flag in ["account_history_in_rocksdb", "transaction_senders_in_static_files"] {
            set_layout(&tx, serde_json::json!({(flag): true, "unknown": [1]}));
            assert_eq!(
                snapshot_uses_v2(&tx, &selections).unwrap_err().to_string(),
                format!("Unsupported legacy snapshot setting for this download: {flag}=true")
            );
        }
        Ok(())
    }

    #[test]
    fn bodies_checkpoint_preserves_transaction_progress() -> eyre::Result<()> {
        let db = create_test_rw_db();
        let manifest = SnapshotManifest { block: 24, ..empty_manifest() };
        let mut config = Config::default();
        config.prune.segments.bodies_history = Some(PruneMode::Before(13));
        let source = PruneCheckpoint { tx_number: Some(16), ..PruneCheckpoint::pruned_through(15) };
        for (first_tx_num, prior, block, tx_number) in
            [(0, None, 9, None), (8, None, 9, Some(7)), (8, Some(source), 15, Some(16))]
        {
            let tx = db.tx_mut()?;
            tx.put::<tables::BlockBodyIndices>(
                9,
                StoredBlockBodyIndices { first_tx_num, tx_count: 0 },
            )?;
            write_prune_checkpoints_tx(
                &tx,
                &config,
                &BTreeMap::new(),
                &manifest,
                true,
                Some(9),
                prior,
            )?;
            assert_eq!(
                tx.get::<tables::PruneCheckpoints>(PruneSegment::Bodies)?,
                Some(PruneCheckpoint {
                    block_number: Some(block),
                    tx_number,
                    prune_mode: PruneMode::Before(13),
                })
            );
        }
        Ok(())
    }
}
