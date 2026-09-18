//! Builds the [`ChainConfig`] reported by `debug_chainConfig` and `admin_nodeInfo`.

use alloy_genesis::ChainConfig;
use reth_chainspec::{EthChainSpec, EthereumHardfork, EthereumHardforks, ForkCondition};

/// Returns the chain config of the given spec.
///
/// The config starts from the genesis file, which is empty for the bundled specs and can be
/// outdated for custom ones, and is completed with the activations and blob params of the spec's
/// hardforks.
pub(crate) fn chain_config<C>(spec: &C) -> ChainConfig
where
    C: EthChainSpec + EthereumHardforks + ?Sized,
{
    let mut config = ChainConfig {
        chain_id: spec.chain().id(),
        terminal_total_difficulty_passed: spec.final_paris_total_difficulty().is_some(),
        terminal_total_difficulty: spec.ethereum_fork_activation(EthereumHardfork::Paris).ttd(),
        deposit_contract_address: spec.deposit_contract().map(|dc| dc.address),
        ..spec.genesis().config.clone()
    };

    let activation = |fork: EthereumHardfork| match spec.ethereum_fork_activation(fork) {
        ForkCondition::Block(block) => Some(block),
        ForkCondition::TTD { fork_block, .. } => fork_block,
        ForkCondition::Timestamp(ts) => Some(ts),
        ForkCondition::Never => None,
    };

    macro_rules! set_block_or_time {
        ($( $field:ident => $fork:ident,)*) => {
            $(
                // the hardforks are the source of truth, but keep the genesis value if the fork
                // is unknown to the spec
                config.$field = activation(EthereumHardfork::$fork).or(config.$field);
            )*
        };
    }

    set_block_or_time!(
        homestead_block => Homestead,
        dao_fork_block => Dao,
        eip150_block => Tangerine,
        eip155_block => SpuriousDragon,
        eip158_block => SpuriousDragon,
        byzantium_block => Byzantium,
        constantinople_block => Constantinople,
        petersburg_block => Petersburg,
        istanbul_block => Istanbul,
        muir_glacier_block => MuirGlacier,
        berlin_block => Berlin,
        london_block => London,
        arrow_glacier_block => ArrowGlacier,
        gray_glacier_block => GrayGlacier,
        shanghai_time => Shanghai,
        cancun_time => Cancun,
        prague_time => Prague,
        osaka_time => Osaka,
        amsterdam_time => Amsterdam,
        bogota_time => Bogota,
        bpo1_time => Bpo1,
        bpo2_time => Bpo2,
        bpo3_time => Bpo3,
        bpo4_time => Bpo4,
        bpo5_time => Bpo5,
    );

    if config.dao_fork_block.is_some() {
        config.dao_fork_support = true;
    }

    let blob_forks = [EthereumHardfork::Cancun, EthereumHardfork::Prague, EthereumHardfork::Osaka]
        .iter()
        .chain(EthereumHardfork::bpo_variants());
    for fork in blob_forks {
        if let ForkCondition::Timestamp(ts) = spec.ethereum_fork_activation(*fork) &&
            let Some(params) = spec.blob_params_at_timestamp(ts)
        {
            config.blob_schedule.insert(fork.name().to_lowercase(), params);
        }
    }

    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::{ChainSpec, HOODI, MAINNET, SEPOLIA};

    #[test]
    fn mainnet_config_is_derived_from_hardforks() {
        let config = chain_config(&*MAINNET);

        assert_eq!(config.chain_id, 1);
        assert_eq!(config.homestead_block, Some(1_150_000));
        assert_eq!(config.dao_fork_block, Some(1_920_000));
        assert!(config.dao_fork_support);
        assert_eq!(config.london_block, Some(12_965_000));
        assert_eq!(config.shanghai_time, Some(1_681_338_455));
        assert_eq!(config.cancun_time, Some(1_710_338_135));
        assert_eq!(config.prague_time, Some(1_746_612_311));
        assert_eq!(
            config.osaka_time,
            MAINNET.ethereum_fork_activation(EthereumHardfork::Osaka).as_timestamp()
        );
        assert_eq!(
            config.bpo1_time,
            MAINNET.ethereum_fork_activation(EthereumHardfork::Bpo1).as_timestamp()
        );
        assert!(config.terminal_total_difficulty_passed);
        assert!(config.terminal_total_difficulty.is_some());
        assert_eq!(
            config.deposit_contract_address,
            MAINNET.deposit_contract().map(|dc| dc.address)
        );
        assert_eq!(
            config.blob_schedule.get("cancun").map(|p| p.max_blob_count),
            MAINNET.blob_params_at_timestamp(config.cancun_time.unwrap()).map(|p| p.max_blob_count)
        );
        assert!(config.blob_schedule.contains_key("prague"));
        assert!(config.blob_schedule.contains_key("osaka"));
        assert_eq!(config.blob_schedule.contains_key("bpo1"), config.bpo1_time.is_some());
    }

    #[test]
    fn sepolia_config_reports_its_chain_id() {
        let config = chain_config(&*SEPOLIA);
        assert_eq!(config.chain_id, SEPOLIA.chain().id());
        assert_eq!(config.shanghai_time, Some(1_677_557_088));
        assert!(config.blob_schedule.contains_key("cancun"));
    }

    #[test]
    fn hoodi_config_includes_forks_missing_from_genesis() {
        let config = chain_config(&*HOODI);
        let osaka = HOODI.ethereum_fork_activation(EthereumHardfork::Osaka).as_timestamp();
        assert!(osaka.is_some());
        assert_eq!(config.osaka_time, osaka);
        assert!(config.blob_schedule.contains_key("osaka"));
    }

    #[test]
    fn genesis_config_fields_are_kept() {
        let mut genesis = MAINNET.genesis().clone();
        genesis.config.chain_id = 1337;
        genesis.config.extra_fields.insert("custom".to_string(), serde_json::json!(true));
        let spec = ChainSpec::from(genesis);

        let config = chain_config(&spec);
        assert_eq!(config.chain_id, 1337);
        assert_eq!(config.extra_fields.get("custom"), Some(&serde_json::json!(true)));
    }
}
