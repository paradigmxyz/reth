use crate::download::manifest::{ComponentSelection, SnapshotComponentType};
use reth_config::config::{Config, PruneConfig};
use reth_db::{tables, Database, DatabaseEnv};
use reth_db_api::transaction::{DbTx, DbTxMut};
use reth_prune_types::{PruneCheckpoint, PruneMode, PruneSegment};
use std::{collections::BTreeMap, path::Path};
use tracing::info;

/// Minimum blocks to keep for receipts, matching `--minimal` prune settings.
const MINIMUM_RECEIPTS_DISTANCE: u64 = 64;

/// Minimum blocks to keep for history/bodies, matching `--minimal` prune settings
/// (`MINIMUM_UNWIND_SAFE_DISTANCE`).
const MINIMUM_HISTORY_DISTANCE: u64 = 10064;

/// Generates an appropriate [`Config`] based on which snapshot components were downloaded.
///
/// The generated prune config mirrors reth's `--minimal` prune settings for components
/// that weren't downloaded:
/// - Missing transactions: prune sender recovery, transaction lookup, and bodies history
/// - Missing receipts: prune receipts to last 64 blocks
/// - Missing changesets: prune account/storage history to last 10,064 blocks
pub fn config_for_components(selected: &[SnapshotComponentType]) -> Config {
    let has_txs = selected.contains(&SnapshotComponentType::Transactions);
    let has_receipts = selected.contains(&SnapshotComponentType::Receipts);
    let has_changesets = selected.contains(&SnapshotComponentType::AccountChangesets) ||
        selected.contains(&SnapshotComponentType::StorageChangesets);

    let mut config = Config::default();

    // Only configure pruning if the user didn't download everything
    if !has_txs || !has_receipts || !has_changesets {
        let mut prune = PruneConfig::default();

        if !has_txs {
            prune.segments.transaction_lookup = Some(PruneMode::Full);
            prune.segments.sender_recovery = Some(PruneMode::Full);
            prune.segments.bodies_history = Some(PruneMode::Distance(MINIMUM_HISTORY_DISTANCE));
        }

        if !has_receipts {
            prune.segments.receipts = Some(PruneMode::Distance(MINIMUM_RECEIPTS_DISTANCE));
        }

        if !has_changesets {
            prune.segments.account_history = Some(PruneMode::Distance(MINIMUM_HISTORY_DISTANCE));
            prune.segments.storage_history = Some(PruneMode::Distance(MINIMUM_HISTORY_DISTANCE));
        }

        config.prune = prune;
    }

    config
}

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

/// Writes prune checkpoints to the database for segments that are configured for pruning.
///
/// After a modular download, data that wasn't downloaded doesn't exist in the DB. Without
/// checkpoints, the pruner would start from block 0 and try to prune non-existent data.
/// Setting checkpoints to the snapshot block tells the pruner "everything up to this block
/// is already in the expected pruned state."
///
/// The `snapshot_block` should be the block number from the manifest.
pub fn write_prune_checkpoints(
    db: &DatabaseEnv,
    config: &Config,
    snapshot_block: u64,
) -> eyre::Result<()> {
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

    let tx = db.tx_mut()?;

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

    tx.commit()?;
    Ok(())
}

/// Returns a human-readable summary of what the prune config does.
pub fn describe_prune_config(selected: &[SnapshotComponentType]) -> Vec<String> {
    let has_txs = selected.contains(&SnapshotComponentType::Transactions);
    let has_receipts = selected.contains(&SnapshotComponentType::Receipts);
    let has_changesets = selected.contains(&SnapshotComponentType::AccountChangesets) ||
        selected.contains(&SnapshotComponentType::StorageChangesets);

    let mut lines = Vec::new();

    if has_txs && has_receipts && has_changesets {
        lines.push("Full archive node — no pruning configured".to_string());
        return lines;
    }

    lines.push("[prune.segments]".to_string());

    if !has_txs {
        lines.push("transaction_lookup = \"full\"".to_string());
        lines.push("sender_recovery = \"full\"".to_string());
        lines.push(format!("bodies_history = {{ distance = {MINIMUM_HISTORY_DISTANCE} }}"));
    }

    if !has_receipts {
        lines.push(format!("receipts = {{ distance = {MINIMUM_RECEIPTS_DISTANCE} }}"));
    }

    if !has_changesets {
        lines.push(format!("account_history = {{ distance = {MINIMUM_HISTORY_DISTANCE} }}"));
        lines.push(format!("storage_history = {{ distance = {MINIMUM_HISTORY_DISTANCE} }}"));
    }

    lines
}

/// Generates a [`Config`] from per-component range selections.
///
/// Each component's selection directly maps to prune settings:
/// - `All`: no pruning for that data category
/// - `Distance(n)`: prune to keep last `n` blocks
/// - `None`: fully prune (or minimal distance for segments that require it)
pub fn config_for_selections(
    selections: &BTreeMap<SnapshotComponentType, ComponentSelection>,
) -> Config {
    let tx_sel = selections
        .get(&SnapshotComponentType::Transactions)
        .copied()
        .unwrap_or(ComponentSelection::None);
    let receipt_sel = selections
        .get(&SnapshotComponentType::Receipts)
        .copied()
        .unwrap_or(ComponentSelection::None);
    let account_cs_sel = selections
        .get(&SnapshotComponentType::AccountChangesets)
        .copied()
        .unwrap_or(ComponentSelection::None);
    let storage_cs_sel = selections
        .get(&SnapshotComponentType::StorageChangesets)
        .copied()
        .unwrap_or(ComponentSelection::None);

    let mut config = Config::default();

    let tx_mode = selection_to_prune_mode(tx_sel);
    let receipt_mode = selection_to_prune_mode(receipt_sel);
    let account_cs_mode = selection_to_prune_mode(account_cs_sel);
    let storage_cs_mode = selection_to_prune_mode(storage_cs_sel);

    if tx_mode.is_some() ||
        receipt_mode.is_some() ||
        account_cs_mode.is_some() ||
        storage_cs_mode.is_some()
    {
        let mut prune = PruneConfig::default();

        if let Some(mode) = tx_mode {
            prune.segments.sender_recovery = Some(mode);
            prune.segments.transaction_lookup = Some(mode);
            prune.segments.bodies_history = Some(mode);
        }

        if let Some(mode) = receipt_mode {
            prune.segments.receipts = Some(mode);
        }

        if let Some(mode) = account_cs_mode {
            prune.segments.account_history = Some(mode);
        }

        if let Some(mode) = storage_cs_mode {
            prune.segments.storage_history = Some(mode);
        }

        config.prune = prune;
    }

    config
}

/// Converts a [`ComponentSelection`] to an optional [`PruneMode`].
fn selection_to_prune_mode(sel: ComponentSelection) -> Option<PruneMode> {
    match sel {
        ComponentSelection::All => None,
        ComponentSelection::Distance(d) => Some(PruneMode::Distance(d)),
        ComponentSelection::None => Some(PruneMode::Full),
    }
}

/// Human-readable prune config summary from per-component selections.
pub fn describe_prune_config_from_selections(
    selections: &BTreeMap<SnapshotComponentType, ComponentSelection>,
) -> Vec<String> {
    let config = config_for_selections(selections);
    let segments = &config.prune.segments;

    // Check if everything is default (no pruning)
    if *segments == Default::default() {
        return vec!["Full archive node — no pruning configured".to_string()];
    }

    let mut lines = vec!["[prune.segments]".to_string()];

    if let Some(mode) = &segments.sender_recovery {
        lines.push(format!("sender_recovery = {}", format_mode(mode)));
    }
    if let Some(mode) = &segments.transaction_lookup {
        lines.push(format!("transaction_lookup = {}", format_mode(mode)));
    }
    if let Some(mode) = &segments.bodies_history {
        lines.push(format!("bodies_history = {}", format_mode(mode)));
    }
    if let Some(mode) = &segments.receipts {
        lines.push(format!("receipts = {}", format_mode(mode)));
    }
    if let Some(mode) = &segments.account_history {
        lines.push(format!("account_history = {}", format_mode(mode)));
    }
    if let Some(mode) = &segments.storage_history {
        lines.push(format!("storage_history = {}", format_mode(mode)));
    }

    lines
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

    #[test]
    fn state_only_prunes_everything() {
        let selected = vec![SnapshotComponentType::State];
        let config = config_for_components(&selected);

        assert_eq!(config.prune.segments.transaction_lookup, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.sender_recovery, Some(PruneMode::Full));
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
    fn minimal_components_mirrors_minimal_flag() {
        let selected: Vec<_> =
            SnapshotComponentType::ALL.iter().copied().filter(|ty| ty.is_minimal()).collect();
        let config = config_for_components(&selected);

        // Minimal downloads txs + changesets, so only receipts should be pruned
        assert_eq!(config.prune.segments.transaction_lookup, None);
        assert_eq!(config.prune.segments.sender_recovery, None);
        assert_eq!(config.prune.segments.bodies_history, None);
        assert_eq!(
            config.prune.segments.receipts,
            Some(PruneMode::Distance(MINIMUM_RECEIPTS_DISTANCE))
        );
        assert_eq!(config.prune.segments.account_history, None);
        assert_eq!(config.prune.segments.storage_history, None);
    }

    #[test]
    fn all_components_no_pruning() {
        let selected = SnapshotComponentType::ALL.to_vec();
        let config = config_for_components(&selected);

        assert_eq!(config.prune.segments.transaction_lookup, None);
        assert_eq!(config.prune.segments.sender_recovery, None);
        assert_eq!(config.prune.segments.bodies_history, None);
        assert_eq!(config.prune.segments.receipts, None);
        assert_eq!(config.prune.segments.account_history, None);
        assert_eq!(config.prune.segments.storage_history, None);
    }

    #[test]
    fn txs_only_keeps_tx_lookup() {
        let selected = vec![SnapshotComponentType::State, SnapshotComponentType::Transactions];
        let config = config_for_components(&selected);

        assert_eq!(config.prune.segments.transaction_lookup, None);
        assert_eq!(config.prune.segments.sender_recovery, None);
        assert_eq!(config.prune.segments.bodies_history, None);
        assert_eq!(
            config.prune.segments.receipts,
            Some(PruneMode::Distance(MINIMUM_RECEIPTS_DISTANCE))
        );
        assert_eq!(
            config.prune.segments.account_history,
            Some(PruneMode::Distance(MINIMUM_HISTORY_DISTANCE))
        );
    }

    #[test]
    fn receipts_only_keeps_receipts() {
        let selected = vec![SnapshotComponentType::State, SnapshotComponentType::Receipts];
        let config = config_for_components(&selected);

        assert_eq!(config.prune.segments.transaction_lookup, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.receipts, None);
    }

    #[test]
    fn describe_state_only() {
        let selected = vec![SnapshotComponentType::State];
        let desc = describe_prune_config(&selected);
        assert!(desc.contains(&"transaction_lookup = \"full\"".to_string()));
        assert!(desc.contains(&format!("receipts = {{ distance = {MINIMUM_RECEIPTS_DISTANCE} }}")));
        assert!(
            desc.contains(&format!("bodies_history = {{ distance = {MINIMUM_HISTORY_DISTANCE} }}"))
        );
    }

    #[test]
    fn describe_all() {
        let selected = SnapshotComponentType::ALL.to_vec();
        let desc = describe_prune_config(&selected);
        assert_eq!(desc.len(), 1);
        assert!(desc[0].contains("no pruning"));
    }

    #[test]
    fn write_prune_checkpoints_sets_all_segments() {
        let dir = tempfile::tempdir().unwrap();
        let db = reth_db::init_db(dir.path(), reth_db::mdbx::DatabaseArguments::default()).unwrap();

        let selected = vec![SnapshotComponentType::State];
        let config = config_for_components(&selected);
        let snapshot_block = 21_000_000;

        write_prune_checkpoints(&db, &config, snapshot_block).unwrap();

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
    fn write_prune_checkpoints_skips_when_no_pruning() {
        let dir = tempfile::tempdir().unwrap();
        let db = reth_db::init_db(dir.path(), reth_db::mdbx::DatabaseArguments::default()).unwrap();

        let selected = SnapshotComponentType::ALL.to_vec();
        let config = config_for_components(&selected);

        write_prune_checkpoints(&db, &config, 21_000_000).unwrap();

        // No segments should have checkpoints since nothing is pruned
        let tx = db.tx().unwrap();
        for segment in PruneSegment::variants() {
            assert!(
                tx.get::<tables::PruneCheckpoints>(segment).unwrap().is_none(),
                "expected no checkpoint for {segment}"
            );
        }
    }

    #[test]
    fn selections_all_no_pruning() {
        let mut selections = BTreeMap::new();
        for ty in SnapshotComponentType::ALL {
            selections.insert(ty, ComponentSelection::All);
        }
        let config = config_for_selections(&selections);
        assert_eq!(config.prune.segments.transaction_lookup, None);
        assert_eq!(config.prune.segments.sender_recovery, None);
        assert_eq!(config.prune.segments.receipts, None);
        assert_eq!(config.prune.segments.account_history, None);
        assert_eq!(config.prune.segments.storage_history, None);
    }

    #[test]
    fn selections_none_prunes_full() {
        let mut selections = BTreeMap::new();
        selections.insert(SnapshotComponentType::State, ComponentSelection::All);
        selections.insert(SnapshotComponentType::Headers, ComponentSelection::All);
        // Everything else omitted → defaults to None → Full prune
        let config = config_for_selections(&selections);
        assert_eq!(config.prune.segments.transaction_lookup, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.sender_recovery, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.bodies_history, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.receipts, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.account_history, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.storage_history, Some(PruneMode::Full));
    }

    #[test]
    fn selections_distance_maps_to_prune_distance() {
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
        let config = config_for_selections(&selections);

        assert_eq!(config.prune.segments.transaction_lookup, Some(PruneMode::Distance(10_064)));
        assert_eq!(config.prune.segments.sender_recovery, Some(PruneMode::Distance(10_064)));
        assert_eq!(config.prune.segments.bodies_history, Some(PruneMode::Distance(10_064)));
        assert_eq!(config.prune.segments.receipts, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.account_history, Some(PruneMode::Distance(10_064)));
        assert_eq!(config.prune.segments.storage_history, Some(PruneMode::Distance(10_064)));
    }

    #[test]
    fn describe_selections_all_archive() {
        let mut selections = BTreeMap::new();
        for ty in SnapshotComponentType::ALL {
            selections.insert(ty, ComponentSelection::All);
        }
        let desc = describe_prune_config_from_selections(&selections);
        assert_eq!(desc.len(), 1);
        assert!(desc[0].contains("no pruning"));
    }

    #[test]
    fn describe_selections_with_distances() {
        let mut selections = BTreeMap::new();
        selections.insert(SnapshotComponentType::State, ComponentSelection::All);
        selections.insert(SnapshotComponentType::Headers, ComponentSelection::All);
        selections
            .insert(SnapshotComponentType::Transactions, ComponentSelection::Distance(10_064));
        selections.insert(SnapshotComponentType::Receipts, ComponentSelection::None);
        let desc = describe_prune_config_from_selections(&selections);
        assert!(desc.contains(&"[prune.segments]".to_string()));
        assert!(desc.contains(&"sender_recovery = { distance = 10064 }".to_string()));
        assert!(desc.contains(&"receipts = \"full\"".to_string()));
    }
}
