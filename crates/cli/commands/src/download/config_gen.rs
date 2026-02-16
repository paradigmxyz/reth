use crate::download::manifest::SnapshotComponentType;
use reth_config::config::{Config, PruneConfig};
use reth_prune_types::PruneMode;
use std::path::Path;
use tracing::info;

/// Generates an appropriate [`Config`] based on which snapshot components were downloaded.
///
/// If a data category wasn't downloaded, the corresponding prune segments are set to avoid
/// indexing or serving that data. Transaction lookup and sender recovery are pruned whenever
/// transaction bodies are not present since the lookup index is useless without the underlying
/// data.
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
        }

        if !has_receipts {
            prune.segments.receipts = Some(PruneMode::Full);
        }

        if !has_changesets {
            prune.segments.account_history = Some(PruneMode::Distance(10064));
            prune.segments.storage_history = Some(PruneMode::Distance(10064));
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
    }

    if !has_receipts {
        lines.push("receipts = \"full\"".to_string());
    }

    if !has_changesets {
        lines.push("account_history = { distance = 10064 }".to_string());
        lines.push("storage_history = { distance = 10064 }".to_string());
    }

    lines
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
        assert_eq!(config.prune.segments.receipts, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.account_history, Some(PruneMode::Distance(10064)));
        assert_eq!(config.prune.segments.storage_history, Some(PruneMode::Distance(10064)));
    }

    #[test]
    fn all_components_no_pruning() {
        let selected = SnapshotComponentType::ALL.to_vec();
        let config = config_for_components(&selected);

        assert_eq!(config.prune.segments.transaction_lookup, None);
        assert_eq!(config.prune.segments.sender_recovery, None);
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
        assert_eq!(config.prune.segments.receipts, Some(PruneMode::Full));
        assert_eq!(config.prune.segments.account_history, Some(PruneMode::Distance(10064)));
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
        assert!(desc.contains(&"receipts = \"full\"".to_string()));
    }

    #[test]
    fn describe_all() {
        let selected = SnapshotComponentType::ALL.to_vec();
        let desc = describe_prune_config(&selected);
        assert_eq!(desc.len(), 1);
        assert!(desc[0].contains("no pruning"));
    }
}
