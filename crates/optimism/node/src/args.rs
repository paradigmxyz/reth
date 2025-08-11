//! Additional Node command arguments.

//! clap [Args](clap::Args) for optimism rollup configuration

use op_alloy_consensus::interop::SafetyLevel;
use reth_optimism_forks::OpHardfork;
use reth_optimism_txpool::supervisor::DEFAULT_SUPERVISOR_URL;

/// Parameters for rollup configuration
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
#[command(next_help_heading = "Rollup")]
pub struct RollupArgs {
    /// Endpoint for the sequencer mempool (can be both HTTP and WS)
    #[arg(long = "rollup.sequencer", visible_aliases = ["rollup.sequencer-http", "rollup.sequencer-ws"])]
    pub sequencer: Option<String>,

    /// Disable transaction pool gossip
    #[arg(long = "rollup.disable-tx-pool-gossip")]
    pub disable_txpool_gossip: bool,

    /// By default the pending block equals the latest block
    /// to save resources and not leak txs from the tx-pool,
    /// this flag enables computing of the pending block
    /// from the tx-pool instead.
    ///
    /// If `compute_pending_block` is not enabled, the payload builder
    /// will use the payload attributes from the latest block. Note
    /// that this flag is not yet functional.
    #[arg(long = "rollup.compute-pending-block")]
    pub compute_pending_block: bool,

    /// enables discovery v4 if provided
    #[arg(long = "rollup.discovery.v4", default_value = "false")]
    pub discovery_v4: bool,

    /// Enable transaction conditional support on sequencer
    #[arg(long = "rollup.enable-tx-conditional", default_value = "false")]
    pub enable_tx_conditional: bool,

    /// HTTP endpoint for the supervisor
    #[arg(
        long = "rollup.supervisor-http",
        value_name = "SUPERVISOR_HTTP_URL",
        default_value = DEFAULT_SUPERVISOR_URL
    )]
    pub supervisor_http: String,

    /// Safety level for the supervisor
    #[arg(
        long = "rollup.supervisor-safety-level",
        default_value_t = SafetyLevel::CrossUnsafe,
    )]
    pub supervisor_safety_level: SafetyLevel,

    /// Optional headers to use when connecting to the sequencer.
    #[arg(long = "rollup.sequencer-headers", requires = "sequencer")]
    pub sequencer_headers: Vec<String>,

    /// RPC endpoint for historical data.
    #[arg(
        long = "rollup.historicalrpc",
        alias = "rollup.historical-rpc",
        value_name = "HISTORICAL_HTTP_URL"
    )]
    pub historical_rpc: Option<String>,

    /// Minimum suggested priority fee (tip) in wei, default `1_000_000`
    #[arg(long, default_value_t = 1_000_000)]
    pub min_suggested_priority_fee: u64,

    /// Override Canyon OP hardfork activation timestamp
    #[arg(long = "override.canyon", value_name = "TIMESTAMP")]
    pub canyon: Option<u64>,

    /// Override Ecotone OP hardfork activation timestamp
    #[arg(long = "override.ecotone", value_name = "TIMESTAMP")]
    pub ecotone: Option<u64>,

    /// Override Fjord OP hardfork activation timestamp
    #[arg(long = "override.fjord", value_name = "TIMESTAMP")]
    pub fjord: Option<u64>,

    /// Override Granite OP hardfork activation timestamp
    #[arg(long = "override.granite", value_name = "TIMESTAMP")]
    pub granite: Option<u64>,

    /// Override Holocene OP hardfork activation timestamp
    #[arg(long = "override.holocene", value_name = "TIMESTAMP")]
    pub holocene: Option<u64>,

    /// Override Isthmus OP hardfork activation timestamp
    #[arg(long = "override.isthmus", value_name = "TIMESTAMP")]
    pub isthmus: Option<u64>,

    /// Override Jovian OP hardfork activation timestamp
    #[arg(long = "override.jovian", value_name = "TIMESTAMP")]
    pub jovian: Option<u64>,

    /// Override Interop OP hardfork activation timestamp
    #[arg(long = "override.interop", value_name = "TIMESTAMP")]
    pub interop: Option<u64>,
}

impl Default for RollupArgs {
    fn default() -> Self {
        Self {
            sequencer: None,
            disable_txpool_gossip: false,
            compute_pending_block: false,
            discovery_v4: false,
            enable_tx_conditional: false,
            supervisor_http: DEFAULT_SUPERVISOR_URL.to_string(),
            supervisor_safety_level: SafetyLevel::CrossUnsafe,
            sequencer_headers: Vec::new(),
            historical_rpc: None,
            min_suggested_priority_fee: 1_000_000,
            canyon: None,
            ecotone: None,
            fjord: None,
            granite: None,
            holocene: None,
            isthmus: None,
            jovian: None,
            interop: None,
        }
    }
}

impl RollupArgs {
    /// Get the OP hardfork overrides as a vector of (hardfork, timestamp) pairs.
    pub fn op_hardforks_as_vec(&self) -> Vec<(OpHardfork, u64)> {
        let mut overrides = Vec::new();

        if let Some(ts) = self.canyon {
            overrides.push((OpHardfork::Canyon, ts));
        }
        if let Some(ts) = self.ecotone {
            overrides.push((OpHardfork::Ecotone, ts));
        }
        if let Some(ts) = self.fjord {
            overrides.push((OpHardfork::Fjord, ts));
        }
        if let Some(ts) = self.granite {
            overrides.push((OpHardfork::Granite, ts));
        }
        if let Some(ts) = self.holocene {
            overrides.push((OpHardfork::Holocene, ts));
        }
        if let Some(ts) = self.isthmus {
            overrides.push((OpHardfork::Isthmus, ts));
        }
        if let Some(ts) = self.jovian {
            overrides.push((OpHardfork::Jovian, ts));
        }
        if let Some(ts) = self.interop {
            overrides.push((OpHardfork::Interop, ts));
        }

        overrides
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Args, Parser};

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn test_parse_optimism_default_args() {
        let default_args = RollupArgs::default();
        let args = CommandParser::<RollupArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }

    #[test]
    fn test_parse_optimism_compute_pending_block_args() {
        let expected_args = RollupArgs { compute_pending_block: true, ..Default::default() };
        let args =
            CommandParser::<RollupArgs>::parse_from(["reth", "--rollup.compute-pending-block"])
                .args;
        assert_eq!(args, expected_args);
    }

    #[test]
    fn test_parse_optimism_discovery_v4_args() {
        let expected_args = RollupArgs { discovery_v4: true, ..Default::default() };
        let args = CommandParser::<RollupArgs>::parse_from(["reth", "--rollup.discovery.v4"]).args;
        assert_eq!(args, expected_args);
    }

    #[test]
    fn test_parse_optimism_sequencer_http_args() {
        let expected_args =
            RollupArgs { sequencer: Some("http://host:port".into()), ..Default::default() };
        let args = CommandParser::<RollupArgs>::parse_from([
            "reth",
            "--rollup.sequencer-http",
            "http://host:port",
        ])
        .args;
        assert_eq!(args, expected_args);
    }

    #[test]
    fn test_parse_optimism_disable_txpool_args() {
        let expected_args = RollupArgs { disable_txpool_gossip: true, ..Default::default() };
        let args =
            CommandParser::<RollupArgs>::parse_from(["reth", "--rollup.disable-tx-pool-gossip"])
                .args;
        assert_eq!(args, expected_args);
    }

    #[test]
    fn test_parse_optimism_enable_tx_conditional() {
        let expected_args = RollupArgs { enable_tx_conditional: true, ..Default::default() };
        let args =
            CommandParser::<RollupArgs>::parse_from(["reth", "--rollup.enable-tx-conditional"])
                .args;
        assert_eq!(args, expected_args);
    }

    #[test]
    fn test_parse_optimism_many_args() {
        let expected_args = RollupArgs {
            disable_txpool_gossip: true,
            compute_pending_block: true,
            enable_tx_conditional: true,
            sequencer: Some("http://host:port".into()),
            ..Default::default()
        };
        let args = CommandParser::<RollupArgs>::parse_from([
            "reth",
            "--rollup.disable-tx-pool-gossip",
            "--rollup.compute-pending-block",
            "--rollup.enable-tx-conditional",
            "--rollup.sequencer-http",
            "http://host:port",
        ])
        .args;
        assert_eq!(args, expected_args);
    }

    #[test]
    fn test_parse_optimism_hardfork_overrides() {
        let expected_args = RollupArgs {
            canyon: Some(1),
            ecotone: Some(2),
            fjord: Some(3),
            granite: Some(4),
            holocene: Some(5),
            isthmus: Some(6),
            jovian: Some(7),
            interop: Some(8),
            ..Default::default()
        };
        let args = CommandParser::<RollupArgs>::parse_from([
            "reth",
            "--override.canyon",
            "1",
            "--override.ecotone",
            "2",
            "--override.fjord",
            "3",
            "--override.granite",
            "4",
            "--override.holocene",
            "5",
            "--override.isthmus",
            "6",
            "--override.jovian",
            "7",
            "--override.interop",
            "8",
        ])
        .args;
        assert_eq!(args, expected_args);
    }
}
