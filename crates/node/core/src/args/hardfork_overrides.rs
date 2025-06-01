use clap::Args;
use reth_chainspec::{ChainHardforks, ChainSpec};
use reth_ethereum_forks::{EthereumHardfork, ForkCondition};
use std::collections::HashMap;

/// Command line arguments for overriding Ethereum hardfork configurations.
///
/// These arguments allow users to override the default timestamps for various
/// Ethereum hardforks, enabling custom network configurations and testing scenarios.
#[derive(Debug, Args, Clone)]
#[command(next_help_heading = "Hardfork Overrides")]
pub struct HardforkOverridesArgs {
    /// Override Frontier hardfork timestamp
    #[arg(long = "override.frontier", value_name = "TIMESTAMP")]
    pub override_frontier: Option<u64>,

    /// Override Homestead hardfork timestamp
    #[arg(long = "override.homestead", value_name = "TIMESTAMP")]
    pub override_homestead: Option<u64>,

    /// Override DAO hardfork timestamp
    #[arg(long = "override.dao", value_name = "TIMESTAMP")]
    pub override_dao: Option<u64>,

    /// Override Tangerine Whistle hardfork timestamp
    #[arg(long = "override.tangerine", value_name = "TIMESTAMP")]
    pub override_tangerine: Option<u64>,

    /// Override Spurious Dragon hardfork timestamp
    #[arg(long = "override.spurious", value_name = "TIMESTAMP")]
    pub override_spurious: Option<u64>,

    /// Override Byzantium hardfork timestamp
    #[arg(long = "override.byzantium", value_name = "TIMESTAMP")]
    pub override_byzantium: Option<u64>,

    /// Override Constantinople hardfork timestamp
    #[arg(long = "override.constantinople", value_name = "TIMESTAMP")]
    pub override_constantinople: Option<u64>,

    /// Override Petersburg hardfork timestamp
    #[arg(long = "override.petersburg", value_name = "TIMESTAMP")]
    pub override_petersburg: Option<u64>,

    /// Override Istanbul hardfork timestamp
    #[arg(long = "override.istanbul", value_name = "TIMESTAMP")]
    pub override_istanbul: Option<u64>,

    /// Override Muir Glacier hardfork timestamp
    #[arg(long = "override.muir", value_name = "TIMESTAMP")]
    pub override_muir: Option<u64>,

    /// Override Berlin hardfork timestamp
    #[arg(long = "override.berlin", value_name = "TIMESTAMP")]
    pub override_berlin: Option<u64>,

    /// Override London hardfork timestamp
    #[arg(long = "override.london", value_name = "TIMESTAMP")]
    pub override_london: Option<u64>,

    /// Override Arrow Glacier hardfork timestamp
    #[arg(long = "override.arrow", value_name = "TIMESTAMP")]
    pub override_arrow: Option<u64>,

    /// Override Gray Glacier hardfork timestamp
    #[arg(long = "override.gray", value_name = "TIMESTAMP")]
    pub override_gray: Option<u64>,

    /// Override Paris (The Merge) hardfork timestamp
    #[arg(long = "override.paris", value_name = "TIMESTAMP")]
    pub override_paris: Option<u64>,

    /// Override Shanghai hardfork timestamp
    #[arg(long = "override.shanghai", value_name = "TIMESTAMP")]
    pub override_shanghai: Option<u64>,

    /// Override Cancun hardfork timestamp
    #[arg(long = "override.cancun", value_name = "TIMESTAMP")]
    pub override_cancun: Option<u64>,

    /// Override Prague hardfork timestamp
    #[arg(long = "override.prague", value_name = "TIMESTAMP")]
    pub override_prague: Option<u64>,

    /// Override Osaka hardfork timestamp
    #[arg(long = "override.osaka", value_name = "TIMESTAMP")]
    pub override_osaka: Option<u64>,
}

impl Default for HardforkOverridesArgs {
    fn default() -> Self {
        Self {
            override_frontier: None,
            override_homestead: None,
            override_dao: None,
            override_tangerine: None,
            override_spurious: None,
            override_byzantium: None,
            override_constantinople: None,
            override_petersburg: None,
            override_istanbul: None,
            override_muir: None,
            override_berlin: None,
            override_london: None,
            override_arrow: None,
            override_gray: None,
            override_paris: None,
            override_shanghai: None,
            override_cancun: None,
            override_prague: None,
            override_osaka: None,
        }
    }
}

impl HardforkOverridesArgs {
    /// Returns a mapping of hardfork names to their override timestamps.
    ///
    /// All hardfork names (as strings) are mapped to the corresponding
    /// `Option<u64>` stored in this struct. If a field is `None`, it means
    /// no override was provided for that hardfork.
    pub fn get_overrides_map(&self) -> HashMap<&'static str, Option<u64>> {
        let mut map = HashMap::new();

        // Gather (key, value) pairs into a small array, then insert them.
        let pairs: [(&'static str, Option<u64>); 19] = [
            ("frontier", self.override_frontier),
            ("homestead", self.override_homestead),
            ("dao", self.override_dao),
            ("tangerine", self.override_tangerine),
            ("spurious", self.override_spurious),
            ("byzantium", self.override_byzantium),
            ("constantinople", self.override_constantinople),
            ("petersburg", self.override_petersburg),
            ("istanbul", self.override_istanbul),
            ("muir", self.override_muir),
            ("berlin", self.override_berlin),
            ("london", self.override_london),
            ("arrow", self.override_arrow),
            ("gray", self.override_gray),
            ("paris", self.override_paris),
            ("shanghai", self.override_shanghai),
            ("cancun", self.override_cancun),
            ("prague", self.override_prague),
            ("osaka", self.override_osaka),
        ];

        for (name, ts_opt) in pairs {
            map.insert(name, ts_opt);
        }

        map
    }

    /// Gets the override timestamp for a specific hardfork (by name).
    ///
    /// Returns `Some(timestamp)` if the user provided `--override.<name>=<timestamp>`,
    /// or `None` otherwise.
    pub fn get_override(&self, hardfork: &str) -> Option<u64> {
        self.get_overrides_map().get(hardfork).copied().flatten()
    }

    /// Applies all non‐None overrides in this struct to the given `ChainHardforks`.
    ///
    /// Any hardfork for which the user passed `--override.<name>=<ts>` will
    /// be inserted (or replaced) with `ForkCondition::Timestamp(ts)`.
    pub fn apply_overrides_to_hardforks(&self, hardforks: &mut ChainHardforks) {
        let overrides = self.get_overrides_map();

        for (name, timestamp_opt) in overrides {
            if let Some(ts) = timestamp_opt {
                if let Ok(hf) = name.parse::<EthereumHardfork>() {
                    hardforks.insert(hf, ForkCondition::Timestamp(ts));
                }
            }
        }
    }

    /// Clones the given `ChainSpec`, applies all overrides to its `.hardforks`,
    /// and returns the modified clone. The original `spec` remains untouched.
    pub fn apply_overrides_to_chainspec(&self, spec: &ChainSpec) -> ChainSpec {
        let mut cloned = spec.clone();
        self.apply_overrides_to_hardforks(&mut cloned.hardforks);
        cloned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use reth_chainspec::MAINNET;

    /// Helper type for parsing command line arguments
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn test_override_prague() {
        let args = CommandParser::<HardforkOverridesArgs>::parse_from([
            "reth node",
            "--override.prague=1234567890",
        ])
        .args;

        assert_eq!(args.get_override("prague"), Some(1234567890));
    }

    #[test]
    fn test_multiple_overrides() {
        let args = CommandParser::<HardforkOverridesArgs>::parse_from([
            "reth node",
            "--override.prague=1234567890",
            "--override.shanghai=9876543210",
        ])
        .args;

        assert_eq!(args.get_override("prague"), Some(1234567890));
        assert_eq!(args.get_override("shanghai"), Some(9876543210));
    }

    #[test]
    fn test_no_override() {
        let args = CommandParser::<HardforkOverridesArgs>::parse_from(["reth node"]).args;

        assert_eq!(args.get_override("prague"), None);
        assert_eq!(args.get_override("shanghai"), None);
    }

    #[test]
    fn test_override_osaka() {
        let args = CommandParser::<HardforkOverridesArgs>::parse_from([
            "reth node",
            "--override.osaka=1234567890",
        ])
        .args;

        assert_eq!(args.get_override("osaka"), Some(1234567890));
    }

    #[test]
    fn test_apply_overrides_updates_chainspec() {
        let args = CommandParser::<HardforkOverridesArgs>::parse_from([
            "reth",
            "--override.prague=1720000000",
        ])
        .args;

        let updated = args.apply_overrides_to_chainspec(&MAINNET);
        assert_eq!(updated.fork(EthereumHardfork::Prague), ForkCondition::Timestamp(1720000000));
    }

    #[test]
    fn test_overrides_map_contains_all_hardforks() {
        let args = HardforkOverridesArgs::default();
        let overrides = args.get_overrides_map();

        for &hf_name in &[
            "frontier",
            "homestead",
            "dao",
            "tangerine",
            "spurious",
            "byzantium",
            "constantinople",
            "petersburg",
            "istanbul",
            "muir",
            "berlin",
            "london",
            "arrow",
            "gray",
            "paris",
            "shanghai",
            "cancun",
            "prague",
            "osaka",
        ] {
            assert!(overrides.contains_key(hf_name));
        }
    }
}
