use alloy_chains::Chain;
use alloy_genesis::ChainConfig;
use derive_more::{Deref, DerefMut, Into};
#[cfg(feature = "optimism")]
use reth_ethereum_forks::OptimismHardfork;
use reth_ethereum_forks::{ConfigureHardforks, EthereumHardfork, ForkCondition, Hardfork};

#[cfg(feature = "optimism")]
use crate::spec::OptimismGenesisInfo;
use crate::{BaseFeeParamsKind, ChainSpec};

/// Wrapper of [`Hardfork`] trait object.
// todo: remove when `BaseFeeParamsKind` is moved to new crate `reth-chainspec-types`
#[derive(Deref, Debug, DerefMut, Clone, Into)]
pub struct AnyHardfork(Box<dyn Hardfork>);

impl Hardfork for AnyHardfork {
    fn name(&self) -> &'static str {
        self.0.name()
    }
}

/// Trait representing type configuring a chain spec.
pub trait EthChainSpec: Send + Sync + Unpin + 'static {
    /// Chain hardforks.
    type Hardfork: Clone;

    /// Chain id.
    fn chain(&self) -> Chain;

    /// Returns base fee params.
    // todo: better suited on chain spec builder.
    fn base_fee_params(config: &ChainConfig) -> BaseFeeParamsKind;
}

impl EthChainSpec for ChainSpec {
    type Hardfork = AnyHardfork;

    fn chain(&self) -> Chain {
        self.chain
    }

    /// Returns base fee params.
    #[allow(unreachable_code)]
    fn base_fee_params(_config: &ChainConfig) -> BaseFeeParamsKind {
        #[cfg(feature = "optimism")]
        {
            return OptimismGenesisInfo::extract_from(_config).base_fee_params
        }

        BaseFeeParamsKind::default()
    }
}

impl ConfigureHardforks for AnyHardfork {
    #[allow(unreachable_code)]
    fn super_chain_mainnet_hardforks() -> impl Iterator<Item = (Self, ForkCondition)> {
        #[cfg(feature = "optimism")]
        {
            return OptimismHardfork::op_mainnet()
                .forks_iter()
                .map(|(hf, opt)| (Self(dyn_clone::clone_box(hf)), opt))
                .collect::<Vec<_>>()
                .into_iter()
        }

        EthereumHardfork::hardforks(Chain::mainnet())
            .iter()
            .map(|(hf, opt)| (Self(Box::new(hf) as Box<dyn Hardfork>), *opt))
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn init_block_hardforks(
        config: &ChainConfig,
    ) -> impl IntoIterator<Item = (Self, ForkCondition)> {
        #[cfg(feature = "optimism")]
        let genesis_info = OptimismGenesisInfo::extract_from(config)
            .optimism_chain_info
            .genesis_info
            .unwrap_or_default();

        [
            (EthereumHardfork::Homestead.boxed(), config.homestead_block),
            (EthereumHardfork::Dao.boxed(), config.dao_fork_block),
            (EthereumHardfork::Tangerine.boxed(), config.eip150_block),
            (EthereumHardfork::SpuriousDragon.boxed(), config.eip155_block),
            (EthereumHardfork::Byzantium.boxed(), config.byzantium_block),
            (EthereumHardfork::Constantinople.boxed(), config.constantinople_block),
            (EthereumHardfork::Petersburg.boxed(), config.petersburg_block),
            (EthereumHardfork::Istanbul.boxed(), config.istanbul_block),
            (EthereumHardfork::MuirGlacier.boxed(), config.muir_glacier_block),
            (EthereumHardfork::Berlin.boxed(), config.berlin_block),
            (EthereumHardfork::London.boxed(), config.london_block),
            (EthereumHardfork::ArrowGlacier.boxed(), config.arrow_glacier_block),
            (EthereumHardfork::GrayGlacier.boxed(), config.gray_glacier_block),
            #[cfg(feature = "optimism")]
            (OptimismHardfork::Bedrock.boxed(), genesis_info.bedrock_block),
        ]
        .into_iter()
        .filter_map(|(hardfork, opt)| {
            opt.map(|block| (Self(hardfork), ForkCondition::Block(block)))
        })
    }

    fn init_paris(config: &ChainConfig) -> Option<(Self, ForkCondition)> {
        let ttd = config.terminal_total_difficulty?;
        Some((
            Self(EthereumHardfork::Paris.boxed()),
            ForkCondition::TTD { total_difficulty: ttd, fork_block: config.merge_netsplit_block },
        ))
    }

    fn init_time_hardforks(
        config: &ChainConfig,
    ) -> impl IntoIterator<Item = (Self, ForkCondition)> {
        #[cfg(feature = "optimism")]
        let genesis_info = OptimismGenesisInfo::extract_from(config)
            .optimism_chain_info
            .genesis_info
            .unwrap_or_default();

        [
            (EthereumHardfork::Shanghai.boxed(), config.shanghai_time),
            (EthereumHardfork::Cancun.boxed(), config.cancun_time),
            (EthereumHardfork::Prague.boxed(), config.prague_time),
            #[cfg(feature = "optimism")]
            (OptimismHardfork::Regolith.boxed(), genesis_info.regolith_time),
            #[cfg(feature = "optimism")]
            (OptimismHardfork::Canyon.boxed(), genesis_info.canyon_time),
            #[cfg(feature = "optimism")]
            (OptimismHardfork::Ecotone.boxed(), genesis_info.ecotone_time),
            #[cfg(feature = "optimism")]
            (OptimismHardfork::Fjord.boxed(), genesis_info.fjord_time),
            #[cfg(feature = "optimism")]
            (OptimismHardfork::Granite.boxed(), genesis_info.granite_time),
        ]
        .into_iter()
        .filter_map(|(hardfork, time)| {
            time.map(|time| (Self(hardfork), ForkCondition::Timestamp(time)))
        })
    }
}
