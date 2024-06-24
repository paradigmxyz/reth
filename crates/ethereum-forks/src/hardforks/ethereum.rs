use crate::{
    hardforks::{ChainHardforks, Hardforks},
    EthereumHardfork, ForkCondition,
};
use alloy_chains::Chain;

/// Helper methods for Ethereum forks.
pub trait EthereumHardforks: Hardforks {
    /// Convenience method to check if [`EthereumHardfork::Shanghai`] is active at a given
    /// timestamp.
    fn is_shanghai_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(EthereumHardfork::Shanghai, timestamp)
    }

    /// Convenience method to check if [`EthereumHardfork::Cancun`] is active at a given timestamp.
    fn is_cancun_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(EthereumHardfork::Cancun, timestamp)
    }

    /// Convenience method to check if [`EthereumHardfork::Prague`] is active at a given timestamp.
    fn is_prague_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(EthereumHardfork::Prague, timestamp)
    }

    /// Convenience method to check if [`EthereumHardfork::Byzantium`] is active at a given block
    /// number.
    fn is_byzantium_active_at_block(&self, block_number: u64) -> bool {
        self.fork(EthereumHardfork::Byzantium).active_at_block(block_number)
    }

    /// Convenience method to check if [`EthereumHardfork::SpuriousDragon`] is active at a given
    /// block number.
    fn is_spurious_dragon_active_at_block(&self, block_number: u64) -> bool {
        self.fork(EthereumHardfork::SpuriousDragon).active_at_block(block_number)
    }

    /// Convenience method to check if [`EthereumHardfork::Homestead`] is active at a given block
    /// number.
    fn is_homestead_active_at_block(&self, block_number: u64) -> bool {
        self.fork(EthereumHardfork::Homestead).active_at_block(block_number)
    }

    /// The Paris hardfork (merge) is activated via block number. If we have knowledge of the block,
    /// this function will return true if the block number is greater than or equal to the Paris
    /// (merge) block.
    fn is_paris_active_at_block(&self, block_number: u64) -> Option<bool> {
        match self.fork(EthereumHardfork::Paris) {
            ForkCondition::Block(paris_block) => Some(block_number >= paris_block),
            ForkCondition::TTD { fork_block, .. } => {
                fork_block.map(|paris_block| block_number >= paris_block)
            }
            _ => None,
        }
    }
}

impl EthereumHardforks for ChainHardforks {}

/// Trait that implements fork activation information helpers for [`ChainHardforks`].
pub trait EthereumActivations {
    /// Retrieves the activation block for the specified hardfork on the given chain.
    fn activation_block(&self, fork: EthereumHardfork, chain: Chain) -> Option<u64>;

    /// Retrieves the activation timestamp for the specified hardfork on the given chain.
    fn activation_timestamp(&self, fork: EthereumHardfork, chain: Chain) -> Option<u64>;

    /// Retrieves the activation block for the specified hardfork on the Ethereum mainnet.
    fn mainnet_activation_block(fork: EthereumHardfork) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match fork {
            EthereumHardfork::Frontier => Some(0),
            EthereumHardfork::Homestead => Some(1150000),
            EthereumHardfork::Dao => Some(1920000),
            EthereumHardfork::Tangerine => Some(2463000),
            EthereumHardfork::SpuriousDragon => Some(2675000),
            EthereumHardfork::Byzantium => Some(4370000),
            EthereumHardfork::Constantinople | EthereumHardfork::Petersburg => Some(7280000),
            EthereumHardfork::Istanbul => Some(9069000),
            EthereumHardfork::MuirGlacier => Some(9200000),
            EthereumHardfork::Berlin => Some(12244000),
            EthereumHardfork::London => Some(12965000),
            EthereumHardfork::ArrowGlacier => Some(13773000),
            EthereumHardfork::GrayGlacier => Some(15050000),
            EthereumHardfork::Paris => Some(15537394),
            EthereumHardfork::Shanghai => Some(17034870),
            EthereumHardfork::Cancun => Some(19426587),

            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Sepolia testnet.
    fn sepolia_activation_block(fork: EthereumHardfork) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match fork {
            EthereumHardfork::Paris => Some(1735371),
            EthereumHardfork::Shanghai => Some(2990908),
            EthereumHardfork::Cancun => Some(5187023),
            EthereumHardfork::Frontier |
            EthereumHardfork::Homestead |
            EthereumHardfork::Dao |
            EthereumHardfork::Tangerine |
            EthereumHardfork::SpuriousDragon |
            EthereumHardfork::Byzantium |
            EthereumHardfork::Constantinople |
            EthereumHardfork::Petersburg |
            EthereumHardfork::Istanbul |
            EthereumHardfork::MuirGlacier |
            EthereumHardfork::Berlin |
            EthereumHardfork::London |
            EthereumHardfork::ArrowGlacier |
            EthereumHardfork::GrayGlacier => Some(0),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Arbitrum Sepolia testnet.
    fn arbitrum_sepolia_activation_block(fork: EthereumHardfork) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match fork {
            EthereumHardfork::Frontier |
            EthereumHardfork::Homestead |
            EthereumHardfork::Dao |
            EthereumHardfork::Tangerine |
            EthereumHardfork::SpuriousDragon |
            EthereumHardfork::Byzantium |
            EthereumHardfork::Constantinople |
            EthereumHardfork::Petersburg |
            EthereumHardfork::Istanbul |
            EthereumHardfork::MuirGlacier |
            EthereumHardfork::Berlin |
            EthereumHardfork::London |
            EthereumHardfork::ArrowGlacier |
            EthereumHardfork::GrayGlacier |
            EthereumHardfork::Paris => Some(0),
            EthereumHardfork::Shanghai => Some(10653737),
            // Hardfork::ArbOS11 => Some(10653737),
            EthereumHardfork::Cancun => Some(18683405),
            // Hardfork::ArbOS20Atlas => Some(18683405),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Arbitrum One mainnet.
    fn arbitrum_activation_block(fork: EthereumHardfork) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match fork {
            EthereumHardfork::Frontier |
            EthereumHardfork::Homestead |
            EthereumHardfork::Dao |
            EthereumHardfork::Tangerine |
            EthereumHardfork::SpuriousDragon |
            EthereumHardfork::Byzantium |
            EthereumHardfork::Constantinople |
            EthereumHardfork::Petersburg |
            EthereumHardfork::Istanbul |
            EthereumHardfork::MuirGlacier |
            EthereumHardfork::Berlin |
            EthereumHardfork::London |
            EthereumHardfork::ArrowGlacier |
            EthereumHardfork::GrayGlacier |
            EthereumHardfork::Paris => Some(0),
            EthereumHardfork::Shanghai => Some(184097479),
            // Hardfork::ArbOS11 => Some(184097479),
            EthereumHardfork::Cancun => Some(190301729),
            // Hardfork::ArbOS20Atlas => Some(190301729),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the holesky testnet.
    fn holesky_activation_block(fork: EthereumHardfork) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match fork {
            EthereumHardfork::Dao |
            EthereumHardfork::Tangerine |
            EthereumHardfork::SpuriousDragon |
            EthereumHardfork::Byzantium |
            EthereumHardfork::Constantinople |
            EthereumHardfork::Petersburg |
            EthereumHardfork::Istanbul |
            EthereumHardfork::MuirGlacier |
            EthereumHardfork::Berlin |
            EthereumHardfork::London |
            EthereumHardfork::ArrowGlacier |
            EthereumHardfork::GrayGlacier |
            EthereumHardfork::Paris => Some(0),
            EthereumHardfork::Shanghai => Some(6698),
            EthereumHardfork::Cancun => Some(894733),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Ethereum mainnet.
    fn mainnet_activation_timestamp(fork: EthereumHardfork) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match fork {
            EthereumHardfork::Frontier => Some(1438226773),
            EthereumHardfork::Homestead => Some(1457938193),
            EthereumHardfork::Dao => Some(1468977640),
            EthereumHardfork::Tangerine => Some(1476753571),
            EthereumHardfork::SpuriousDragon => Some(1479788144),
            EthereumHardfork::Byzantium => Some(1508131331),
            EthereumHardfork::Constantinople | EthereumHardfork::Petersburg => Some(1551340324),
            EthereumHardfork::Istanbul => Some(1575807909),
            EthereumHardfork::MuirGlacier => Some(1577953849),
            EthereumHardfork::Berlin => Some(1618481223),
            EthereumHardfork::London => Some(1628166822),
            EthereumHardfork::ArrowGlacier => Some(1639036523),
            EthereumHardfork::GrayGlacier => Some(1656586444),
            EthereumHardfork::Paris => Some(1663224162),
            EthereumHardfork::Shanghai => Some(1681338455),
            EthereumHardfork::Cancun => Some(1710338135),

            // upcoming hardforks
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Sepolia testnet.
    fn sepolia_activation_timestamp(fork: EthereumHardfork) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match fork {
            EthereumHardfork::Frontier |
            EthereumHardfork::Homestead |
            EthereumHardfork::Dao |
            EthereumHardfork::Tangerine |
            EthereumHardfork::SpuriousDragon |
            EthereumHardfork::Byzantium |
            EthereumHardfork::Constantinople |
            EthereumHardfork::Petersburg |
            EthereumHardfork::Istanbul |
            EthereumHardfork::MuirGlacier |
            EthereumHardfork::Berlin |
            EthereumHardfork::London |
            EthereumHardfork::ArrowGlacier |
            EthereumHardfork::GrayGlacier |
            EthereumHardfork::Paris => Some(1633267481),
            EthereumHardfork::Shanghai => Some(1677557088),
            EthereumHardfork::Cancun => Some(1706655072),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Holesky testnet.
    fn holesky_activation_timestamp(fork: EthereumHardfork) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match fork {
            EthereumHardfork::Shanghai => Some(1696000704),
            EthereumHardfork::Cancun => Some(1707305664),
            EthereumHardfork::Frontier |
            EthereumHardfork::Homestead |
            EthereumHardfork::Dao |
            EthereumHardfork::Tangerine |
            EthereumHardfork::SpuriousDragon |
            EthereumHardfork::Byzantium |
            EthereumHardfork::Constantinople |
            EthereumHardfork::Petersburg |
            EthereumHardfork::Istanbul |
            EthereumHardfork::MuirGlacier |
            EthereumHardfork::Berlin |
            EthereumHardfork::London |
            EthereumHardfork::ArrowGlacier |
            EthereumHardfork::GrayGlacier |
            EthereumHardfork::Paris => Some(1695902100),
            _ => None,
        }
    }
}

impl EthereumActivations for ChainHardforks {
    /// Retrieves the activation block for the specified hardfork on the given chain.
    fn activation_block(&self, fork: EthereumHardfork, chain: Chain) -> Option<u64> {
        if chain == Chain::mainnet() {
            return Self::mainnet_activation_block(fork)
        }
        if chain == Chain::sepolia() {
            return Self::sepolia_activation_block(fork)
        }
        if chain == Chain::holesky() {
            return Self::holesky_activation_block(fork)
        }

        None
    }

    /// Retrieves the activation timestamp for the specified hardfork on the given chain.
    fn activation_timestamp(&self, fork: EthereumHardfork, chain: Chain) -> Option<u64> {
        if chain == Chain::mainnet() {
            return Self::mainnet_activation_timestamp(fork)
        }
        if chain == Chain::sepolia() {
            return Self::sepolia_activation_timestamp(fork)
        }
        if chain == Chain::holesky() {
            return Self::holesky_activation_timestamp(fork)
        }

        None
    }
}
