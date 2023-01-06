use crate::{BlockNumber, ForkFilter, ForkHash, ForkId, MAINNET_GENESIS, ChainSpec};
use std::str::FromStr;

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

    pub fn all_concrete_forks<'a, CS>(chain_spec: &'a CS) -> Vec<ConcreteHardfork<'a, CS>> {
        Self::all_forks().into_iter().map(|f| ConcreteHardfork::new(chain_spec, f)).collect()
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

impl<CS: ChainSpec> From<(&CS, BlockNumber)> for Hardfork {
    fn from((chain_spec , num): (&CS, BlockNumber)) -> Self {
        match num {
            _i if num < chain_spec.fork_id(&Hardfork::Frontier).next => Hardfork::Frontier,
            _i if num < chain_spec.fork_id(&Hardfork::Dao).next => Hardfork::Dao,
            _i if num < chain_spec.fork_id(&Hardfork::Homestead).next => Hardfork::Homestead,
            _i if num < chain_spec.fork_id(&Hardfork::Tangerine).next => Hardfork::Tangerine,
            _i if num < chain_spec.fork_id(&Hardfork::SpuriousDragon).next => Hardfork::SpuriousDragon,
            _i if num < chain_spec.fork_id(&Hardfork::Byzantium).next => Hardfork::Byzantium,
            _i if num < chain_spec.fork_id(&Hardfork::Constantinople).next => Hardfork::Constantinople,
            _i if num < chain_spec.fork_id(&Hardfork::Istanbul).next => Hardfork::Istanbul,
            _i if num < chain_spec.fork_id(&Hardfork::Muirglacier).next => Hardfork::Muirglacier,
            _i if num < chain_spec.fork_id(&Hardfork::Berlin).next => Hardfork::Berlin,
            _i if num < chain_spec.fork_id(&Hardfork::London).next => Hardfork::London,
            _i if num < chain_spec.fork_id(&Hardfork::ArrowGlacier).next => Hardfork::ArrowGlacier,
    
            _ => Hardfork::Latest,
        }
    }
}

pub struct ConcreteHardfork<'a, CS> {
    chain_spec: &'a CS,
    fork: Hardfork,
}

impl<'a, CS> ConcreteHardfork<'a, CS> {
    pub fn new(chain_spec: &'a CS, fork: Hardfork) -> Self{
        Self {
            chain_spec,
            fork
        }
    }
}

impl<'a, CS: ChainSpec> From<ConcreteHardfork<'a, CS>> for BlockNumber {
    fn from(value: ConcreteHardfork<'a, CS>) -> Self {
        value.chain_spec.fork_block(&value.fork)
    }
}

#[cfg(test)]
mod tests {
    use crate::{forkid::ForkHash, hardfork::Hardfork, MainnetSpec, ChainSpec};
    use crc::crc32;

    #[test]
    fn test_hardfork_blocks() {
        let mainnet = MainnetSpec::default();
        
        let hf: Hardfork = (&mainnet, 12_965_000u64).into();
        assert_eq!(hf, Hardfork::London);

        let hf: Hardfork = (&mainnet, 4370000u64).into();
        assert_eq!(hf, Hardfork::Byzantium);

        let hf: Hardfork = (&mainnet, 12244000u64).into();
        assert_eq!(hf, Hardfork::Berlin);
    }

    #[test]
    // this test checks that the fork hash assigned to forks accurately map to the fork_id method
    fn test_forkhash_from_fork_blocks() {
        let mainnet = MainnetSpec::default();
        // set the genesis hash
        let genesis =
            hex::decode("d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap();

        // set the frontier forkhash
        let mut curr_forkhash = ForkHash(crc32::checksum_ieee(&genesis[..]).to_be_bytes());

        // now we go through enum members
        let frontier_forkid = mainnet.fork_id(&Hardfork::Frontier);
        assert_eq!(curr_forkhash, frontier_forkid.hash);

        // list of the above hardforks
        let hardforks = Hardfork::all_forks();

        // check that the curr_forkhash we compute matches the output of each fork_id returned
        for hardfork in hardforks {
            curr_forkhash += mainnet.fork_block(&hardfork);
            assert_eq!(curr_forkhash, mainnet.fork_id(&hardfork).hash);
        }
    }
}