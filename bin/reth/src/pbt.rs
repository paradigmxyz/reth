//! Explicit opt-in for the reference PBT devnet backend.

use alloy_primitives::B256;
use clap::Args;
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_node_pbt::{
    activation_timestamp, configure_chain, CatchupLimits, DecodeLimits, MigrationFiles,
    MigrationSource, PbtConfig,
};
use std::{num::NonZeroUsize, path::PathBuf, sync::Arc};

#[derive(Debug, Default, Args)]
pub(crate) struct PbtArgs {
    /// Enable experimental BLAKE3 PBT state commitments using genesis config.binaryTrieTime.
    #[arg(long, conflicts_with = "snap_v2")]
    pbt: bool,
    /// Hash dense PBT state-update batches in parallel; sparse updates remain incremental.
    #[arg(long = "pbt.parallel", requires = "pbt")]
    parallel: bool,
    /// Irreversibly retire MPT storage after persisted post-PBT finality.
    #[arg(long = "pbt.retire-mpt", env = "PBT_RETIRE_MPT", requires = "pbt")]
    retire_mpt: bool,
    /// Canonical snapshot input; omit to convert retained local MPT state.
    #[arg(long = "pbt.migration-snapshot", env = "PBT_MIGRATION_SNAPSHOT", requires_all = ["pbt", "migration_preimages", "migration_anchor"])]
    migration_snapshot: Option<PathBuf>,
    /// Canonical preimages for the finalized migration anchor.
    #[arg(long = "pbt.migration-preimages", env = "PBT_MIGRATION_PREIMAGES", requires_all = ["pbt", "migration_anchor"])]
    migration_preimages: Option<PathBuf>,
    /// Locally finalized pre-PBT migration header hash.
    #[arg(long = "pbt.migration-anchor", env = "PBT_MIGRATION_ANCHOR", requires_all = ["pbt", "migration_preimages"])]
    migration_anchor: Option<B256>,
    /// Maximum onboarding range or staged chunk length (default 1024 blocks).
    #[arg(long = "pbt.catchup-max-blocks", env = "PBT_CATCHUP_MAX_BLOCKS", requires = "pbt")]
    catchup_max_blocks: Option<NonZeroUsize>,
    /// Maximum authenticated BAL bytes per replay chunk (default 64 MiB).
    #[arg(long = "pbt.catchup-max-bal-bytes", env = "PBT_CATCHUP_MAX_BAL_BYTES", requires = "pbt")]
    catchup_max_bal_bytes: Option<NonZeroUsize>,
    /// Estimated full-state cache byte budget (default 1 GiB); zero disables caching.
    #[arg(long = "pbt.cache-max-bytes", env = "PBT_CACHE_MAX_BYTES", requires = "pbt")]
    cache_max_bytes: Option<usize>,
    /// Checkpoints retained below finality, excluding genesis and retirement (default 64).
    #[arg(long = "pbt.checkpoint-history", env = "PBT_CHECKPOINT_HISTORY", requires = "pbt")]
    checkpoint_history: Option<u64>,
    /// Maximum accounts in an untrusted migration artifact (default 1000000).
    #[arg(long = "pbt.import-max-accounts", env = "PBT_IMPORT_MAX_ACCOUNTS", requires = "pbt")]
    import_max_accounts: Option<usize>,
    /// Maximum leaves in an untrusted migration artifact (default 10000000).
    #[arg(long = "pbt.import-max-leaves", env = "PBT_IMPORT_MAX_LEAVES", requires = "pbt")]
    import_max_leaves: Option<usize>,
    /// Maximum single-code byte length in an artifact (default 16 MiB).
    #[arg(long = "pbt.import-max-code-size", env = "PBT_IMPORT_MAX_CODE_SIZE", requires = "pbt")]
    import_max_code_size: Option<usize>,
    /// Maximum aggregate artifact bytecode size (default 256 MiB).
    #[arg(
        long = "pbt.import-max-total-code-size",
        env = "PBT_IMPORT_MAX_TOTAL_CODE_SIZE",
        requires = "pbt"
    )]
    import_max_total_code_size: Option<usize>,
}

impl PbtArgs {
    pub(crate) fn activation(&self, chain: &ChainSpec) -> eyre::Result<Option<u64>> {
        let activation = activation_timestamp(chain)?;
        eyre::ensure!(
            self.pbt || activation.is_none(),
            "this chain configures binaryTrieTime; enable --pbt"
        );
        if self.pbt {
            eyre::ensure!(
                activation.is_some(),
                "--pbt requires config.binaryTrieTime in the genesis file"
            );
            eyre::ensure!(
                chain.is_amsterdam_active_at_timestamp(chain.genesis_timestamp()),
                "--pbt requires Amsterdam/BAL activation at genesis for staged synchronization"
            );
        }
        Ok(activation)
    }

    pub(crate) const fn parallel(&self) -> bool {
        self.parallel
    }

    pub(crate) fn config(&self) -> eyre::Result<PbtConfig> {
        let defaults = PbtConfig::default();
        let import = defaults.import_limits;
        let migration = match (&self.migration_preimages, self.migration_anchor) {
            (Some(preimages), Some(anchor)) => Some(MigrationFiles {
                source: self
                    .migration_snapshot
                    .clone()
                    .map_or(MigrationSource::Mpt, MigrationSource::Snapshot),
                preimages: preimages.clone(),
                anchor,
            }),
            (None, None) if self.migration_snapshot.is_none() => None,
            _ => eyre::bail!("PBT migration requires both preimages and a finalized anchor"),
        };
        Ok(PbtConfig {
            migration,
            catchup: CatchupLimits {
                max_blocks: self.catchup_max_blocks.unwrap_or(defaults.catchup.max_blocks),
                max_bal_bytes: self.catchup_max_bal_bytes.unwrap_or(defaults.catchup.max_bal_bytes),
            },
            import_limits: DecodeLimits {
                max_accounts: self.import_max_accounts.unwrap_or(import.max_accounts),
                max_leaves: self.import_max_leaves.unwrap_or(import.max_leaves),
                max_code_size: self.import_max_code_size.unwrap_or(import.max_code_size),
                max_total_code_size: self
                    .import_max_total_code_size
                    .unwrap_or(import.max_total_code_size),
            },
            cache_max_bytes: self.cache_max_bytes,
            checkpoint_history: self.checkpoint_history.unwrap_or(defaults.checkpoint_history),
            retire_mpt: self.retire_mpt,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PbtChainSpecParser;

impl ChainSpecParser for PbtChainSpecParser {
    type ChainSpec = ChainSpec;

    const SUPPORTED_CHAINS: &'static [&'static str] = EthereumChainSpecParser::SUPPORTED_CHAINS;

    fn parse(value: &str) -> eyre::Result<Arc<ChainSpec>> {
        configure_chain(EthereumChainSpecParser::parse(value)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use reth::cli::Cli;

    #[test]
    fn resource_limits_and_retirement_are_explicit() {
        let mut cli =
            Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from(["reth", "node", "--pbt"]).unwrap();
        assert!(!cli.as_node_command_mut().unwrap().ext.config().unwrap().retire_mpt);
        let mut cli = Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from([
            "reth",
            "node",
            "--pbt",
            "--pbt.retire-mpt",
            "--pbt.cache-max-bytes",
            "0",
            "--pbt.catchup-max-blocks",
            "3",
        ])
        .unwrap();
        let config = cli.as_node_command_mut().unwrap().ext.config().unwrap();
        assert!(config.retire_mpt);
        assert_eq!(config.cache_max_bytes, Some(0));
        assert_eq!(config.catchup.max_blocks.get(), 3);
        for flag in ["--pbt.catchup-max-blocks", "--pbt.catchup-max-bal-bytes"] {
            let error = Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from([
                "reth", "node", "--pbt", flag, "0",
            ])
            .unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        }
        for flag in ["--pbt.cache-max-bytes", "--pbt.import-max-leaves"] {
            for value in ["-1", "many", "184467440737095516160"] {
                assert!(Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from([
                    "reth", "node", "--pbt", flag, value
                ])
                .is_err());
            }
        }
    }

    #[test]
    fn migration_requires_a_complete_finalized_anchor() {
        for flag in ["--pbt.migration-preimages", "--pbt.migration-snapshot"] {
            let error = Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from([
                "reth", "node", "--pbt", flag, "input",
            ])
            .unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::MissingRequiredArgument);
        }
        let mut cli = Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from([
            "reth",
            "node",
            "--pbt",
            "--pbt.migration-preimages",
            "input",
            "--pbt.migration-anchor",
            &B256::ZERO.to_string(),
        ])
        .unwrap();
        assert!(matches!(
            cli.as_node_command_mut().unwrap().ext.config().unwrap().migration.unwrap().source,
            MigrationSource::Mpt
        ));
    }

    #[test]
    fn flag_is_opt_in_and_conflicts_with_mpt_snap_sync() {
        let mut cli = Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from(["reth", "node"]).unwrap();
        assert!(!cli.as_node_command_mut().unwrap().ext.pbt);
        let mut cli =
            Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from(["reth", "node", "--pbt"]).unwrap();
        assert!(cli.as_node_command_mut().unwrap().ext.pbt);
        let error = Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from([
            "reth",
            "node",
            "--pbt",
            "--snap.v2",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn flag_requires_a_shared_activation_and_bal_schedule() {
        let chain = EthereumChainSpecParser::parse("dev").unwrap();
        assert_eq!(PbtArgs::default().activation(&chain).unwrap(), None);
        assert_eq!(
            PbtArgs { pbt: true, ..Default::default() }.activation(&chain).unwrap_err().to_string(),
            "--pbt requires config.binaryTrieTime in the genesis file"
        );
        let mut chain = (*chain).clone();
        chain.genesis.config.extra_fields.insert("binaryTrieTime".into(), serde_json::json!(0));
        assert_eq!(
            PbtArgs::default().activation(&chain).unwrap_err().to_string(),
            "this chain configures binaryTrieTime; enable --pbt"
        );
        assert_eq!(
            PbtArgs { pbt: true, ..Default::default() }.activation(&chain).unwrap_err().to_string(),
            "--pbt requires Amsterdam/BAL activation at genesis for staged synchronization"
        );
    }

    #[test]
    fn parallel_requires_pbt_and_is_opt_in() {
        let mut cli =
            Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from(["reth", "node", "--pbt"]).unwrap();
        assert!(!cli.as_node_command_mut().unwrap().ext.parallel());
        let mut cli = Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from([
            "reth",
            "node",
            "--pbt",
            "--pbt.parallel",
        ])
        .unwrap();
        assert!(cli.as_node_command_mut().unwrap().ext.parallel());
        let error =
            Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from(["reth", "node", "--pbt.parallel"])
                .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }
}
