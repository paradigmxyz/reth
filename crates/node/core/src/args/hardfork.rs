//! clap [Args](clap::Args) for hardfork activation block overrides

use reth_chainspec::{EthereumHardfork, ForkCondition};
/// Parameters for hardfork activation block overrides
#[derive(Default, Debug, Clone, clap::Args, PartialEq)]
#[command(next_help_heading = "Hardfork activation block overrides")]
pub struct HardforkOverrideArgs {
    /// Override the block number after which the Frontier hardfork is activated
    #[arg(long = "override.frontier")]
    pub frontier: Option<u64>,

    /// Override the block number after which the Homestead hardfork is activated
    #[arg(long = "override.homestead")]
    pub homestead: Option<u64>,

    /// Override the block number after which the Dao hardfork is activated
    #[arg(long = "override.dao")]
    pub dao: Option<u64>,

    /// Override the block number after which the Tangerine hardfork is activated
    #[arg(long = "override.tangerine")]
    pub tangerine: Option<u64>,

    /// Override the block number after which the SpuriousDragon hardfork is activated
    #[arg(long = "override.spuriousdragon")]
    pub spuriousdragon: Option<u64>,

    /// Override the block number after which the Byzantium hardfork is activated
    #[arg(long = "override.byzantium")]
    pub byzantium: Option<u64>,

    /// Override the block number after which the Constantinople hardfork is activated
    #[arg(long = "override.constantinople")]
    pub constantinople: Option<u64>,

    /// Override the block number after which the Petersburg hardfork is activated
    #[arg(long = "override.petersburg")]
    pub petersburg: Option<u64>,

    /// Override the block number after which the Istanbul hardfork is activated
    #[arg(long = "override.istanbul")]
    pub istanbul: Option<u64>,

    /// Override the block number after which the MuirGlacier hardfork is activated
    #[arg(long = "override.muirglacier")]
    pub muirglacier: Option<u64>,

    /// Override the block number after which the Berlin hardfork is activated
    #[arg(long = "override.berlin")]
    pub berlin: Option<u64>,

    /// Override the block number after which the London hardfork is activated
    #[arg(long = "override.london")]
    pub london: Option<u64>,

    /// Override the block number after which the ArrowGlacier hardfork is activated
    #[arg(long = "override.arrowglacier")]
    pub arrowglacier: Option<u64>,

    /// Override the block number after which the Prague hardfork is activated
    #[arg(long = "override.prague")]
    pub prague: Option<u64>,

    /// Override the block number after which the Paris hardfork is activated
    #[arg(long = "override.paris")]
    pub paris: Option<u64>,

    /// Override the block number after which the Shanghai hardfork is activated
    #[arg(long = "override.shanghai")]
    pub shanghai: Option<u64>,

    /// Override the block number after which the Cancun hardfork is activated
    #[arg(long = "override.cancun")]
    pub cancun: Option<u64>,
}

impl HardforkOverrideArgs {
    /// Get the hardfork overrides
    pub fn overrides(&self) -> Vec<(EthereumHardfork, ForkCondition)> {
        let mut hardforks = Vec::new();
        if let Some(frontier) = self.frontier {
            hardforks.push((EthereumHardfork::Frontier, ForkCondition::Block(frontier)));
        }
        if let Some(homestead) = self.homestead {
            hardforks.push((EthereumHardfork::Homestead, ForkCondition::Block(homestead)));
        }
        if let Some(dao) = self.dao {
            hardforks.push((EthereumHardfork::Dao, ForkCondition::Block(dao)));
        }
        if let Some(tangerine) = self.tangerine {
            hardforks.push((EthereumHardfork::Tangerine, ForkCondition::Block(tangerine)));
        }
        if let Some(spuriousdragon) = self.spuriousdragon {
            hardforks
                .push((EthereumHardfork::SpuriousDragon, ForkCondition::Block(spuriousdragon)));
        }
        if let Some(byzantium) = self.byzantium {
            hardforks.push((EthereumHardfork::Byzantium, ForkCondition::Block(byzantium)));
        }
        if let Some(constantinople) = self.constantinople {
            hardforks
                .push((EthereumHardfork::Constantinople, ForkCondition::Block(constantinople)));
        }
        if let Some(petersburg) = self.petersburg {
            hardforks.push((EthereumHardfork::Petersburg, ForkCondition::Block(petersburg)));
        }
        if let Some(istanbul) = self.istanbul {
            hardforks.push((EthereumHardfork::Istanbul, ForkCondition::Block(istanbul)));
        }
        if let Some(muirglacier) = self.muirglacier {
            hardforks.push((EthereumHardfork::MuirGlacier, ForkCondition::Block(muirglacier)));
        }
        if let Some(berlin) = self.berlin {
            hardforks.push((EthereumHardfork::Berlin, ForkCondition::Block(berlin)));
        }
        if let Some(london) = self.london {
            hardforks.push((EthereumHardfork::London, ForkCondition::Block(london)));
        }
        if let Some(arrowglacier) = self.arrowglacier {
            hardforks.push((EthereumHardfork::ArrowGlacier, ForkCondition::Block(arrowglacier)));
        }
        if let Some(prague) = self.prague {
            hardforks.push((EthereumHardfork::Prague, ForkCondition::Block(prague)));
        }
        if let Some(paris) = self.paris {
            hardforks.push((EthereumHardfork::Paris, ForkCondition::Block(paris)));
        }
        if let Some(shanghai) = self.shanghai {
            hardforks.push((EthereumHardfork::Shanghai, ForkCondition::Block(shanghai)));
        }
        if let Some(cancun) = self.cancun {
            hardforks.push((EthereumHardfork::Cancun, ForkCondition::Block(cancun)));
        }
        hardforks
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
    fn test_parse_hardfork_overrides() {
        let expected_args = HardforkOverrideArgs {
            frontier: Some(1),
            homestead: Some(2),
            dao: Some(3),
            tangerine: Some(4),
            spuriousdragon: Some(5),
            byzantium: Some(6),
            constantinople: Some(7),
            petersburg: Some(8),
            istanbul: Some(9),
            muirglacier: Some(10),
            berlin: Some(11),
            london: Some(12),
            arrowglacier: Some(13),
            prague: Some(14),
            paris: Some(15),
            shanghai: Some(16),
            cancun: Some(17),
        };
        let args = CommandParser::<HardforkOverrideArgs>::parse_from([
            "reth",
            "--override.prague",
            "1",
            "--override.homestead",
            "2",
            "--override.dao",
            "3",
            "--override.tangerine",
            "4",
            "--override.spuriousdragon",
            "5",
            "--override.byzantium",
            "6",
            "--override.constantinople",
            "7",
            "--override.petersburg",
            "8",
            "--override.istanbul",
            "9",
            "--override.muirglacier",
            "10",
            "--override.berlin",
            "11",
            "--override.london",
            "12",
            "--override.arrowglacier",
            "13",
            "--override.prague",
            "14",
            "--override.paris",
            "15",
            "--override.shanghai",
            "16",
            "--override.cancun",
            "17",
        ])
        .args;
        assert_eq!(args, expected_args);
    }
}
