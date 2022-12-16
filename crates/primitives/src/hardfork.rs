use crate::{BlockNumber, ForkFilter, ForkHash, ForkId, MAINNET_GENESIS};
use std::str::FromStr;

/// Ethereum mainnet hardforks
#[allow(missing_docs)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
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
    Latest,
}

impl Hardfork {
    /// Get the first block number of the hardfork.
    pub fn fork_block(&self) -> u64 {
        match *self {
            Hardfork::Frontier => 0,
            Hardfork::Homestead => 1150000,
            Hardfork::Dao => 1920000,
            Hardfork::Tangerine => 2463000,
            Hardfork::SpuriousDragon => 2675000,
            Hardfork::Byzantium => 4370000,
            Hardfork::Constantinople | Hardfork::Petersburg => 7280000,
            Hardfork::Istanbul => 9069000,
            Hardfork::Muirglacier => 9200000,
            Hardfork::Berlin => 12244000,
            Hardfork::London => 12965000,
            Hardfork::ArrowGlacier => 13773000,
            Hardfork::GrayGlacier | Hardfork::Latest => 15050000,
        }
    }

    /// Get the EIP-2124 fork id for a given hardfork
    ///
    /// The [`ForkId`](ethereum_forkid::ForkId) includes a CRC32 checksum of the all fork block
    /// numbers from genesis, and the next upcoming fork block number.
    /// If the next fork block number is not yet known, it is set to 0.
    pub fn fork_id(&self) -> ForkId {
        match *self {
            Hardfork::Frontier => {
                ForkId { hash: ForkHash([0xfc, 0x64, 0xec, 0x04]), next: 1150000 }
            }
            Hardfork::Homestead => {
                ForkId { hash: ForkHash([0x97, 0xc2, 0xc3, 0x4c]), next: 1920000 }
            }
            Hardfork::Dao => ForkId { hash: ForkHash([0x91, 0xd1, 0xf9, 0x48]), next: 2463000 },
            Hardfork::Tangerine => {
                ForkId { hash: ForkHash([0x7a, 0x64, 0xda, 0x13]), next: 2675000 }
            }
            Hardfork::SpuriousDragon => {
                ForkId { hash: ForkHash([0x3e, 0xdd, 0x5b, 0x10]), next: 4370000 }
            }
            Hardfork::Byzantium => {
                ForkId { hash: ForkHash([0xa0, 0x0b, 0xc3, 0x24]), next: 7280000 }
            }
            Hardfork::Constantinople | Hardfork::Petersburg => {
                ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 }
            }
            Hardfork::Istanbul => {
                ForkId { hash: ForkHash([0x87, 0x9d, 0x6e, 0x30]), next: 9200000 }
            }
            Hardfork::Muirglacier => {
                ForkId { hash: ForkHash([0xe0, 0x29, 0xe9, 0x91]), next: 12244000 }
            }
            Hardfork::Berlin => ForkId { hash: ForkHash([0x0e, 0xb4, 0x40, 0xf6]), next: 12965000 },
            Hardfork::London => ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 13773000 },
            Hardfork::ArrowGlacier => {
                ForkId { hash: ForkHash([0x20, 0xc3, 0x27, 0xfc]), next: 15050000 }
            }
            Hardfork::Latest | Hardfork::GrayGlacier => {
                // update `next` when another fork block num is known
                ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 0 }
            }
        }
    }

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

    /// This returns all known hardfork block numbers as a vector.
    pub fn all_fork_blocks() -> Vec<BlockNumber> {
        Hardfork::all_forks().iter().map(|f| f.fork_block()).collect()
    }

    /// Creates a [`ForkFilter`](crate::ForkFilter) for the given hardfork.
    ///
    /// **CAUTION**: This assumes the current hardfork's block number is the current head and uses
    /// all known future hardforks to initialize the filter.
    pub fn fork_filter(&self) -> ForkFilter {
        let all_forks = Hardfork::all_forks();
        let future_forks: Vec<BlockNumber> = all_forks
            .iter()
            .filter(|f| f.fork_block() > self.fork_block())
            .map(|f| f.fork_block())
            .collect();

        // this data structure is not chain-agnostic, so we can pass in the constant mainnet
        // genesis
        ForkFilter::new(self.fork_block(), MAINNET_GENESIS, future_forks)
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

impl Default for Hardfork {
    fn default() -> Self {
        Hardfork::Latest
    }
}

impl From<BlockNumber> for Hardfork {
    fn from(num: BlockNumber) -> Hardfork {
        match num {
            _i if num < 1_150_000 => Hardfork::Frontier,
            _i if num < 1_920_000 => Hardfork::Dao,
            _i if num < 2_463_000 => Hardfork::Homestead,
            _i if num < 2_675_000 => Hardfork::Tangerine,
            _i if num < 4_370_000 => Hardfork::SpuriousDragon,
            _i if num < 7_280_000 => Hardfork::Byzantium,
            _i if num < 9_069_000 => Hardfork::Constantinople,
            _i if num < 9_200_000 => Hardfork::Istanbul,
            _i if num < 12_244_000 => Hardfork::Muirglacier,
            _i if num < 12_965_000 => Hardfork::Berlin,
            _i if num < 13_773_000 => Hardfork::London,
            _i if num < 15_050_000 => Hardfork::ArrowGlacier,

            _ => Hardfork::Latest,
        }
    }
}

impl From<Hardfork> for BlockNumber {
    fn from(value: Hardfork) -> Self {
        value.fork_block()
    }
}

#[cfg(test)]
mod tests {
    use crate::{forkid::ForkHash, hardfork::Hardfork};
    use crc::crc32;

    #[test]
    fn test_hardfork_blocks() {
        let hf: Hardfork = 12_965_000u64.into();
        assert_eq!(hf, Hardfork::London);

        let hf: Hardfork = 4370000u64.into();
        assert_eq!(hf, Hardfork::Byzantium);

        let hf: Hardfork = 12244000u64.into();
        assert_eq!(hf, Hardfork::Berlin);
    }

    #[test]
    // this test checks that the fork hash assigned to forks accurately map to the fork_id method
    fn test_forkhash_from_fork_blocks() {
        // set the genesis hash
        let genesis =
            hex::decode("d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap();

        // set the frontier forkhash
        let mut curr_forkhash = ForkHash(crc32::checksum_ieee(&genesis[..]).to_be_bytes());

        // now we go through enum members
        let frontier_forkid = Hardfork::Frontier.fork_id();
        assert_eq!(curr_forkhash, frontier_forkid.hash);

        // list of the above hardforks
        let hardforks = Hardfork::all_forks();

        // check that the curr_forkhash we compute matches the output of each fork_id returned
        for hardfork in hardforks {
            curr_forkhash += hardfork.fork_block();
            assert_eq!(curr_forkhash, hardfork.fork_id().hash);
        }
    }
}
