use serde::{Deserialize, Serialize};

use std::{fmt::Display, str::FromStr};

use crate::{BlockNumber, ChainSpec, ForkFilter, ForkHash, ForkId};

#[allow(missing_docs)]
#[derive(
    Debug, Default, Copy, Clone, Eq, PartialEq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum Hardfork {
    Frontier,
    Homestead,
    Dao,
    Tangerine,
    SpuriousDragon,
    Byzantium,
    Constantinople,
    Petersburg,
    Istanbul,
    Muirglacier,
    Berlin,
    London,
    ArrowGlacier,
    GrayGlacier,
    Shanghai,
    #[default]
    Latest,
}

impl Hardfork {
    /// This returns all known hardforks in order.
    pub fn all_forks() -> Vec<Self> {
        vec![
            Hardfork::Homestead,
            Hardfork::Dao,
            Hardfork::Tangerine,
            Hardfork::SpuriousDragon,
            Hardfork::Byzantium,
            Hardfork::Constantinople, /* petersburg is skipped because it's the same block num
                                       * as constantinople */
            Hardfork::Istanbul,
            Hardfork::Muirglacier,
            Hardfork::Berlin,
            Hardfork::London,
            Hardfork::ArrowGlacier,
            Hardfork::GrayGlacier,
        ]
    }

    /// Compute the forkid for the given [ChainSpec]
    pub fn fork_id(&self, chain_spec: &ChainSpec) -> Option<ForkId> {
        if let Some(fork_block) = chain_spec.fork_block(*self) {
            let mut curr_forkhash = ForkHash::from(chain_spec.genesis_hash());
            let mut curr_block_number = 0;

            for (_, b) in chain_spec.forks_iter() {
                if fork_block >= b {
                    if b != curr_block_number {
                        curr_forkhash += b;
                        curr_block_number = b;
                    }
                } else {
                    return Some(ForkId { hash: curr_forkhash, next: b })
                }
            }
            Some(ForkId { hash: curr_forkhash, next: 0 })
        } else {
            None
        }
    }

    /// Creates a [`ForkFilter`](crate::ForkFilter) for the given hardfork.
    ///
    /// **CAUTION**: This assumes the current hardfork's block number is the current head and uses
    /// all known future hardforks to initialize the filter.
    pub fn fork_filter(&self, chain_spec: &ChainSpec) -> Option<ForkFilter> {
        if let Some(fork_block) = chain_spec.fork_block(*self) {
            let future_forks: Vec<BlockNumber> =
                chain_spec.forks_iter().filter(|(_, b)| b > &fork_block).map(|(_, b)| b).collect();

            // this data structure is not chain-agnostic, so we can pass in the constant mainnet
            // genesis
            Some(ForkFilter::new(fork_block, chain_spec.genesis_hash(), future_forks))
        } else {
            None
        }
    }
}

impl FromStr for Hardfork {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_lowercase();
        let hardfork = match s.as_str() {
            "frontier" | "1" => Hardfork::Frontier,
            "homestead" | "2" => Hardfork::Homestead,
            "dao" | "3" => Hardfork::Dao,
            "tangerine" | "4" => Hardfork::Tangerine,
            "spuriousdragon" | "5" => Hardfork::SpuriousDragon,
            "byzantium" | "6" => Hardfork::Byzantium,
            "constantinople" | "7" => Hardfork::Constantinople,
            "petersburg" | "8" => Hardfork::Petersburg,
            "istanbul" | "9" => Hardfork::Istanbul,
            "muirglacier" | "10" => Hardfork::Muirglacier,
            "berlin" | "11" => Hardfork::Berlin,
            "london" | "12" => Hardfork::London,
            "arrowglacier" | "13" => Hardfork::ArrowGlacier,
            "grayglacier" => Hardfork::GrayGlacier,
            "latest" | "14" => Hardfork::Latest,
            _ => return Err(format!("Unknown hardfork {s}")),
        };
        Ok(hardfork)
    }
}

impl Display for Hardfork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
