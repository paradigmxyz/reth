use clap::Args;
use reth_chainspec::ChainHardforks;
use reth_ethereum_forks::{EthereumHardfork, ForkCondition};

/// Command line arguments for overriding Ethereum hardfork configurations.
///
/// These arguments allow users to override the default timestamps for various
/// Ethereum hardforks, enabling custom network configurations and testing scenarios.
#[derive(Default, Debug, Args, Clone)]
#[command(next_help_heading = "Hardfork Overrides")]
pub struct HardforkOverridesArgs {
    /// Override Frontier hardfork block number
    #[arg(long = "override.frontier")]
    pub override_frontier: Option<u64>,

    /// Override Homestead hardfork block number
    #[arg(long = "override.homestead")]
    pub override_homestead: Option<u64>,

    /// Override DAO hardfork block number
    #[arg(long = "override.dao")]
    pub override_dao: Option<u64>,

    /// Override Tangerine Whistle hardfork block number
    #[arg(long = "override.tangerine")]
    pub override_tangerine: Option<u64>,

    /// Override Spurious Dragon hardfork block number
    #[arg(long = "override.spuriousdragon")]
    pub override_spurious: Option<u64>,

    /// Override Byzantium hardfork block number
    #[arg(long = "override.byzantium")]
    pub override_byzantium: Option<u64>,

    /// Override Constantinople hardfork block number
    #[arg(long = "override.constantinople")]
    pub override_constantinople: Option<u64>,

    /// Override Petersburg hardfork block number
    #[arg(long = "override.petersburg")]
    pub override_petersburg: Option<u64>,

    /// Override Istanbul hardfork block number
    #[arg(long = "override.istanbul")]
    pub override_istanbul: Option<u64>,

    /// Override Muir Glacier hardfork block number
    #[arg(long = "override.muirglacier")]
    pub override_muir: Option<u64>,

    /// Override Berlin hardfork block number
    #[arg(long = "override.berlin")]
    pub override_berlin: Option<u64>,

    /// Override London hardfork block number
    #[arg(long = "override.london")]
    pub override_london: Option<u64>,

    /// Override Arrow Glacier hardfork block number
    #[arg(long = "override.arrowglacier")]
    pub override_arrow: Option<u64>,

    /// Override Gray Glacier hardfork block number
    #[arg(long = "override.grayglacier")]
    pub override_gray: Option<u64>,

    /// Override Paris (The Merge) hardfork timestamp
    #[arg(long = "override.paris")]
    pub override_paris: Option<u64>,

    /// Override Shanghai hardfork timestamp
    #[arg(long = "override.shanghai")]
    pub override_shanghai: Option<u64>,

    /// Override Cancun hardfork timestamp
    #[arg(long = "override.cancun")]
    pub override_cancun: Option<u64>,

    /// Override Prague hardfork timestamp
    #[arg(long = "override.prague")]
    pub override_prague: Option<u64>,

    /// Override Osaka hardfork timestamp
    #[arg(long = "override.osaka")]
    pub override_osaka: Option<u64>,
}

impl HardforkOverridesArgs {
    /// Gets the override condition for a specific hardfork (by name).
    ///
    /// Returns `Some(timestamp/block)` if the user provided `--override.<name>=<timestamp/block>`,
    /// or `None` otherwise.
    pub fn get_override(&self, hardfork: &str) -> Option<u64> {
        match hardfork {
            "frontier" => self.override_frontier,
            "homestead" => self.override_homestead,
            "dao" => self.override_dao,
            "tangerine" => self.override_tangerine,
            "spuriousdragon" => self.override_spurious,
            "byzantium" => self.override_byzantium,
            "constantinople" => self.override_constantinople,
            "petersburg" => self.override_petersburg,
            "istanbul" => self.override_istanbul,
            "muirglacier" => self.override_muir,
            "berlin" => self.override_berlin,
            "london" => self.override_london,
            "arrowglacier" => self.override_arrow,
            "grayglacier" => self.override_gray,
            "paris" => self.override_paris,
            "shanghai" => self.override_shanghai,
            "cancun" => self.override_cancun,
            "prague" => self.override_prague,
            "osaka" => self.override_osaka,
            _ => None,
        }
    }

    /// Applies all non‐None overrides in this struct to the given `ChainHardforks`.
    ///
    /// Any hardfork for which the user passed `--override.<name>=<block/timestamp>` will
    /// be inserted (or replaced) with `ForkCondition::Block(block)` or
    /// `ForkCondition::Timestamp(timestamp)`.
    pub fn apply_overrides_to_hardforks(&self, mut hardforks: ChainHardforks) -> ChainHardforks {
        macro_rules! apply {
            ($field:ident, $hf:ident) => {
                if let Some(ts) = self.$field {
                    let original_condition = hardforks.fork(EthereumHardfork::$hf);
                    let new_condition = match original_condition {
                        ForkCondition::Block(_) => ForkCondition::Block(ts),
                        ForkCondition::Timestamp(_) => ForkCondition::Timestamp(ts),
                        _ => original_condition,
                    };
                    hardforks.insert(EthereumHardfork::$hf, new_condition);
                }
            };
        }

        apply!(override_frontier, Frontier);
        apply!(override_homestead, Homestead);
        apply!(override_dao, Dao);
        apply!(override_tangerine, Tangerine);
        apply!(override_spurious, SpuriousDragon);
        apply!(override_byzantium, Byzantium);
        apply!(override_constantinople, Constantinople);
        apply!(override_petersburg, Petersburg);
        apply!(override_istanbul, Istanbul);
        apply!(override_muir, MuirGlacier);
        apply!(override_berlin, Berlin);
        apply!(override_london, London);
        apply!(override_arrow, ArrowGlacier);
        apply!(override_gray, GrayGlacier);
        apply!(override_paris, Paris);
        apply!(override_shanghai, Shanghai);
        apply!(override_cancun, Cancun);
        apply!(override_prague, Prague);
        apply!(override_osaka, Osaka);

        hardforks
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

        let updated = args.apply_overrides_to_hardforks(MAINNET.hardforks.clone());
        assert_eq!(updated.fork(EthereumHardfork::Prague), ForkCondition::Timestamp(1720000000));
    }
}
