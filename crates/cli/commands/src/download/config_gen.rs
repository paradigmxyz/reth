use crate::download::{
    manifest::{ComponentManifest, ComponentSelection, SnapshotComponentType, SnapshotManifest},
    SelectionPreset,
};
use reth_chainspec::{EthereumHardfork, EthereumHardforks};
use reth_config::config::{BlocksPerFileConfig, Config, PruneConfig, StaticFilesConfig};
use reth_db::tables;
use reth_db_api::transaction::{DbTx, DbTxMut};
use reth_node_core::args::DefaultPruningValues;
use reth_prune_types::{PruneCheckpoint, PruneMode, PruneSegment};
use reth_stages_types::StageCheckpoint;
use std::{collections::BTreeMap, path::Path};
use tracing::info;

/// Minimum blocks to keep for receipts, matching `--minimal` prune settings.
const MINIMUM_RECEIPTS_DISTANCE: u64 = 64;

/// Minimum blocks to keep for history/bodies, matching `--minimal` prune settings
/// (`MINIMUM_UNWIND_SAFE_DISTANCE`).
const MINIMUM_HISTORY_DISTANCE: u64 = 10064;

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

/// Writes prune checkpoints to the provided write transaction.
pub(crate) fn write_prune_checkpoints_tx<Tx>(
    tx: &Tx,
    config: &Config,
    snapshot_block: u64,
) -> eyre::Result<()>
where
    Tx: DbTx + DbTxMut,
{
    let segments = &config.prune.segments;

    // Collect (segment, mode) pairs for all configured prune segments
    let checkpoints: Vec<(PruneSegment, PruneMode)> = [
        (PruneSegment::SenderRecovery, segments.sender_recovery),
        (PruneSegment::TransactionLookup, segments.transaction_lookup),
        (PruneSegment::Receipts, segments.receipts),
        (PruneSegment::AccountHistory, segments.account_history),
        (PruneSegment::StorageHistory, segments.storage_history),
        (PruneSegment::Bodies, segments.bodies_history),
    ]
    .into_iter()
    .filter_map(|(segment, mode)| mode.map(|m| (segment, m)))
    .collect();

    if checkpoints.is_empty() {
        return Ok(());
    }

    // Look up the last tx number for the snapshot block from BlockBodyIndices
    let tx_number =
        tx.get::<tables::BlockBodyIndices>(snapshot_block)?.map(|indices| indices.last_tx_num());

    for (segment, prune_mode) in &checkpoints {
        let checkpoint = PruneCheckpoint {
            block_number: Some(snapshot_block),
            tx_number,
            prune_mode: *prune_mode,
        };

        tx.put::<tables::PruneCheckpoints>(*segment, checkpoint)?;

        info!(target: "reth::cli",
            segment = %segment,
            block = snapshot_block,
            tx = ?tx_number,
            mode = ?prune_mode,
            "Set prune checkpoint"
        );
    }

    Ok(())
}

/// Stage IDs for index stages whose output is stored in RocksDB and is never
/// distributed in snapshots.
const INDEX_STAGE_IDS: [&str; 3] =
    ["TransactionLookup", "IndexAccountHistory", "IndexStorageHistory"];

/// Prune segments that correspond to the index stages.
const INDEX_PRUNE_SEGMENTS: [PruneSegment; 3] =
    [PruneSegment::TransactionLookup, PruneSegment::AccountHistory, PruneSegment::StorageHistory];

/// Resets stage and prune checkpoints for stages whose output is not included
/// in the snapshot inside an existing write transaction.
///
/// A snapshot's mdbx comes from a fully synced node, so it has stage checkpoints
/// at the tip for `TransactionLookup`, `IndexAccountHistory`, and
/// `IndexStorageHistory`. Since we don't distribute the rocksdb indices those
/// stages produced, we must reset their checkpoints to block 0. Otherwise the
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

    // Clear corresponding prune checkpoints so the pruner doesn't inherit
    // state from the source node
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
            ComponentManifest::Single(_) => None,
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

    if is_archive || matches!(preset, Some(SelectionPreset::Archive)) {
        return Config { static_files, ..Default::default() };
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

    let mut config = Config::default();
    let mut prune = PruneConfig::default();

    if senders_sel != ComponentSelection::All {
        prune.segments.sender_recovery = Some(PruneMode::Full);
    }
    prune.segments.transaction_lookup = Some(PruneMode::Full);

    if let Some(mode) = selection_to_prune_mode(tx_sel, Some(MINIMUM_HISTORY_DISTANCE)) {
        prune.segments.bodies_history = Some(mode);
    }

    if let Some(mode) = selection_to_prune_mode(receipt_sel, Some(MINIMUM_RECEIPTS_DISTANCE)) {
        prune.segments.receipts = Some(mode);
    }

    if let Some(mode) = selection_to_prune_mode(account_cs_sel, Some(MINIMUM_HISTORY_DISTANCE)) {
        prune.segments.account_history = Some(mode);
    }

    if let Some(mode) = selection_to_prune_mode(storage_cs_sel, Some(MINIMUM_HISTORY_DISTANCE)) {
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
fn selection_to_prune_mode(
    sel: ComponentSelection,
    min_distance: Option<u64>,
) -> Option<PruneMode> {
    match sel {
        ComponentSelection::All => None,
        ComponentSelection::Distance(d) => {
            Some(PruneMode::Distance(min_distance.map_or(d, |min| d.max(min))))
        }
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
    use reth_db::Database;

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
        }
    }

    #[test]
    fn write_prune_checkpoints_sets_all_segments() {
        let dir = tempfile::tempdir().unwrap();
        let db = reth_db::init_db(dir.path(), reth_db::mdbx::DatabaseArguments::default()).unwrap();

        let mut selections = BTreeMap::new();
        selections.insert(SnapshotComponentType::State, ComponentSelection::All);
        selections.insert(SnapshotComponentType::Headers, ComponentSelection::All);
        let config = config_for_selections(
            &selections,
            &empty_manifest(),
            None,
            None::<&reth_chainspec::ChainSpec>,
        );
        let snapshot_block = 21_000_000;

        {
            let tx = db.tx_mut().unwrap();
            write_prune_checkpoints_tx(&tx, &config, snapshot_block).unwrap();
            tx.commit().unwrap();
        }

        // Verify all expected segments have checkpoints
        let tx = db.tx().unwrap();
        for segment in [
            PruneSegment::SenderRecovery,
            PruneSegment::TransactionLookup,
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
    fn write_prune_checkpoints_archive_no_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let db = reth_db::init_db(dir.path(), reth_db::mdbx::DatabaseArguments::default()).unwrap();

        // Archive node — no pruning configured, so no checkpoints written
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

        {
            let tx = db.tx_mut().unwrap();
            write_prune_checkpoints_tx(&tx, &config, 21_000_000).unwrap();
            tx.commit().unwrap();
        }

        let tx = db.tx().unwrap();
        for segment in [PruneSegment::SenderRecovery, PruneSegment::TransactionLookup] {
            assert!(
                tx.get::<tables::PruneCheckpoints>(segment).unwrap().is_none(),
                "expected no checkpoint for {segment} on archive node"
            );
        }
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
            Some(PruneMode::Distance(MINIMUM_HISTORY_DISTANCE))
        );
        assert_eq!(
            config.prune.segments.receipts,
            Some(PruneMode::Distance(MINIMUM_RECEIPTS_DISTANCE))
        );
        assert_eq!(
            config.prune.segments.account_history,
            Some(PruneMode::Distance(MINIMUM_HISTORY_DISTANCE))
        );
        assert_eq!(
            config.prune.segments.storage_history,
            Some(PruneMode::Distance(MINIMUM_HISTORY_DISTANCE))
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
        assert_eq!(
            config.prune.segments.receipts,
            Some(PruneMode::Distance(MINIMUM_RECEIPTS_DISTANCE))
        );
        assert_eq!(config.prune.segments.account_history, Some(PruneMode::Distance(10_064)));
        assert_eq!(config.prune.segments.storage_history, Some(PruneMode::Distance(10_064)));
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
            Some(PruneMode::Distance(MINIMUM_HISTORY_DISTANCE))
        );
        assert_eq!(
            config.prune.segments.account_history,
            Some(PruneMode::Distance(MINIMUM_HISTORY_DISTANCE))
        );
        assert_eq!(
            config.prune.segments.storage_history,
            Some(PruneMode::Distance(MINIMUM_HISTORY_DISTANCE))
        );

        let paris_block = chain_spec
            .ethereum_fork_activation(EthereumHardfork::Paris)
            .block_number()
            .expect("mainnet Paris block should be known");
        assert_eq!(config.prune.segments.bodies_history, Some(PruneMode::Before(paris_block)));
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
        assert!(desc.contains(&"sender_recovery=\"full\"".to_string()));
        // Bodies follows tx selection
        assert!(desc.contains(&"bodies_history={ distance = 10064 }".to_string()));
        assert!(desc.contains(&"receipts={ distance = 64 }".to_string()));
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

            // Sender recovery checkpoints should be preserved by reset.
            tx.put::<tables::StageCheckpoints>("SenderRecovery".to_string(), tip_checkpoint)
                .unwrap();
            tx.put::<tables::PruneCheckpoints>(
                PruneSegment::SenderRecovery,
                PruneCheckpoint {
                    block_number: Some(24_500_000),
                    tx_number: None,
                    prune_mode: PruneMode::Full,
                },
            )
            .unwrap();
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
    }
}
