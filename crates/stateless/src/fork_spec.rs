// This is here so we don't pull in the EF-tests.
// We need to think more about how we will parse in the chain-spec
use reth_chainspec::{ChainSpec, ChainSpecBuilder};
use serde::{Deserialize, Serialize};

/// Fork specification.
#[derive(Debug, PartialEq, Eq, PartialOrd, Hash, Ord, Clone, Copy, Serialize, Deserialize)]
pub enum ForkSpec {
    /// Frontier
    Frontier,
    /// Frontier to Homestead
    FrontierToHomesteadAt5,
    /// Homestead
    Homestead,
    /// Homestead to Tangerine
    HomesteadToDaoAt5,
    /// Homestead to Tangerine
    HomesteadToEIP150At5,
    /// Tangerine
    EIP150,
    /// Spurious Dragon
    EIP158, // EIP-161: State trie clearing
    /// Spurious Dragon to Byzantium
    EIP158ToByzantiumAt5,
    /// Byzantium
    Byzantium,
    /// Byzantium to Constantinople
    ByzantiumToConstantinopleAt5, // SKIPPED
    /// Byzantium to Constantinople
    ByzantiumToConstantinopleFixAt5,
    /// Constantinople
    Constantinople, // SKIPPED
    /// Constantinople fix
    ConstantinopleFix,
    /// Istanbul
    Istanbul,
    /// Berlin
    Berlin,
    /// Berlin to London
    BerlinToLondonAt5,
    /// London
    London,
    /// Paris aka The Merge
    Merge,
    /// Shanghai
    Shanghai,
    /// Merge EOF test
    #[serde(alias = "Merge+3540+3670")]
    MergeEOF,
    /// After Merge Init Code test
    #[serde(alias = "Merge+3860")]
    MergeMeterInitCode,
    /// After Merge plus new PUSH0 opcode
    #[serde(alias = "Merge+3855")]
    MergePush0,
    /// Cancun
    Cancun,
    /// Prague
    Prague,
}

impl From<ForkSpec> for ChainSpec {
    fn from(fork_spec: ForkSpec) -> Self {
        let spec_builder = ChainSpecBuilder::mainnet();

        match fork_spec {
            ForkSpec::Frontier => spec_builder.frontier_activated(),
            ForkSpec::Homestead | ForkSpec::FrontierToHomesteadAt5 => {
                spec_builder.homestead_activated()
            }
            ForkSpec::EIP150 | ForkSpec::HomesteadToDaoAt5 | ForkSpec::HomesteadToEIP150At5 => {
                spec_builder.tangerine_whistle_activated()
            }
            ForkSpec::EIP158 => spec_builder.spurious_dragon_activated(),
            ForkSpec::Byzantium |
            ForkSpec::EIP158ToByzantiumAt5 |
            ForkSpec::ConstantinopleFix |
            ForkSpec::ByzantiumToConstantinopleFixAt5 => spec_builder.byzantium_activated(),
            ForkSpec::Istanbul => spec_builder.istanbul_activated(),
            ForkSpec::Berlin => spec_builder.berlin_activated(),
            ForkSpec::London | ForkSpec::BerlinToLondonAt5 => spec_builder.london_activated(),
            ForkSpec::Merge |
            ForkSpec::MergeEOF |
            ForkSpec::MergeMeterInitCode |
            ForkSpec::MergePush0 => spec_builder.paris_activated(),
            ForkSpec::Shanghai => spec_builder.shanghai_activated(),
            ForkSpec::Cancun => spec_builder.cancun_activated(),
            ForkSpec::ByzantiumToConstantinopleAt5 | ForkSpec::Constantinople => {
                panic!("Overridden with PETERSBURG")
            }
            ForkSpec::Prague => spec_builder.prague_activated(),
        }
        .build()
    }
}
