use crate::alloc::string::String;
use alloy_chains::NamedChain;
use alloy_genesis::ChainConfig;
use alloy_primitives::{Address, ChainId, B256, U256};
use serde::{Deserialize, Serialize};

// Not all fields are currently used, but they are kept for future use.

/// The chain metadata stored in a superchain toml config file.
/// Referring here as `ChainMetadata` to avoid confusion with `ChainConfig`.
/// Find configs here: `<https://github.com/ethereum-optimism/superchain-registry/tree/main/superchain/configs>`
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ChainMetadata {
    pub name: String,
    pub public_rpc: String,
    pub sequencer_rpc: String,
    pub explorer: String,
    pub superchain_level: i32,
    pub governed_by_optimism: bool,
    pub superchain_time: Option<u64>,
    pub data_availability_type: String,
    pub deployment_tx_hash: Option<B256>,

    pub chain_id: ChainId,
    pub batch_inbox_addr: Address,
    pub block_time: u64,
    pub seq_window_size: u64,
    pub max_sequencer_drift: u64,
    pub gas_paying_token: Option<Address>,
    pub hardforks: HardforkConfig,
    pub optimism: Option<OptimismConfig>,

    pub alt_da: Option<AltDAConfig>,
    pub genesis: GenesisConfig,
    pub roles: RolesConfig,
    pub addresses: AddressesConfig,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct HardforkConfig {
    pub canyon_time: Option<u64>,
    pub delta_time: Option<u64>,
    pub ecotone_time: Option<u64>,
    pub fjord_time: Option<u64>,
    pub granite_time: Option<u64>,
    pub holocene_time: Option<u64>,
    pub isthmus_time: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct OptimismConfig {
    pub eip1559_elasticity: u64,
    pub eip1559_denominator: u64,
    pub eip1559_denominator_canyon: Option<u64>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct AltDAConfig {
    pub da_challenge_contract_address: Address,
    pub da_challenge_window: u64,
    pub da_resolve_window: u64,
    pub da_commitment_type: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct GenesisConfig {
    pub l2_time: u64,
    pub l1: GenesisRef,
    pub l2: GenesisRef,
    pub system_config: SystemConfig,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GenesisRef {
    pub hash: B256,
    pub number: u64,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SystemConfig {
    pub batcher_address: Address,
    pub overhead: B256,
    pub scalar: B256,
    pub gas_limit: u64,
    pub base_fee_scalar: Option<u64>,
    pub blob_base_fee_scalar: Option<u64>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct RolesConfig {
    pub system_config_owner: Option<Address>,
    pub proxy_admin_owner: Option<Address>,
    pub guardian: Option<Address>,
    pub challenger: Option<Address>,
    pub proposer: Option<Address>,
    pub unsafe_block_signer: Option<Address>,
    pub batch_submitter: Option<Address>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct AddressesConfig {
    pub address_manager: Option<Address>,
    pub l1_cross_domain_messenger_proxy: Option<Address>,
    #[serde(rename = "L1ERC721BridgeProxy")]
    pub l1_erc721_bridge_proxy: Option<Address>,
    pub l1_standard_bridge_proxy: Option<Address>,
    pub l2_output_oracle_proxy: Option<Address>,
    #[serde(rename = "OptimismMintableERC20FactoryProxy")]
    pub optimism_mintable_erc20_factory_proxy: Option<Address>,
    pub optimism_portal_proxy: Option<Address>,
    pub system_config_proxy: Option<Address>,
    pub proxy_admin: Option<Address>,
    pub superchain_config: Option<Address>,
    pub anchor_state_registry_proxy: Option<Address>,
    #[serde(rename = "DelayedWETHProxy")]
    pub delayed_weth_proxy: Option<Address>,
    pub dispute_game_factory_proxy: Option<Address>,
    pub fault_dispute_game: Option<Address>,
    #[serde(rename = "MIPS")]
    pub mips: Option<Address>,
    pub permissioned_dispute_game: Option<Address>,
    pub preimage_oracle: Option<Address>,
    pub da_challenge_address: Option<Address>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChainConfigExtraFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bedrock_block: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regolith_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canyon_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecotone_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fjord_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granite_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holocene_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isthmus_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimism: Option<ChainConfigExtraFieldsOptimism>,
}

// Helper struct to serialize field for extra fields in ChainConfig
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChainConfigExtraFieldsOptimism {
    pub eip1559_elasticity: u64,
    pub eip1559_denominator: u64,
    pub eip1559_denominator_canyon: Option<u64>,
}

impl From<&OptimismConfig> for ChainConfigExtraFieldsOptimism {
    fn from(value: &OptimismConfig) -> Self {
        Self {
            eip1559_elasticity: value.eip1559_elasticity,
            eip1559_denominator: value.eip1559_denominator,
            eip1559_denominator_canyon: value.eip1559_denominator_canyon,
        }
    }
}

/// Returns a [`ChainConfig`] filled from [`ChainMetadata`] with extra fields and handling
/// special case for Optimism chain.
// Mimic the behavior from https://github.com/ethereum-optimism/op-geth/blob/35e2c852/params/superchain.go#L26
pub(crate) fn to_genesis_chain_config(chain_config: &ChainMetadata) -> ChainConfig {
    let mut res = ChainConfig {
        chain_id: chain_config.chain_id,
        homestead_block: Some(0),
        dao_fork_block: None,
        dao_fork_support: false,
        eip150_block: Some(0),
        eip155_block: Some(0),
        eip158_block: Some(0),
        byzantium_block: Some(0),
        constantinople_block: Some(0),
        petersburg_block: Some(0),
        istanbul_block: Some(0),
        muir_glacier_block: Some(0),
        berlin_block: Some(0),
        london_block: Some(0),
        arrow_glacier_block: Some(0),
        gray_glacier_block: Some(0),
        merge_netsplit_block: Some(0),
        shanghai_time: chain_config.hardforks.canyon_time, // Shanghai activates with Canyon
        cancun_time: chain_config.hardforks.ecotone_time,  // Cancun activates with Ecotone
        prague_time: chain_config.hardforks.isthmus_time,  // Prague activates with Isthmus
        osaka_time: None,
        terminal_total_difficulty: Some(U256::ZERO),
        terminal_total_difficulty_passed: true,
        ethash: None,
        clique: None,
        ..Default::default()
    };

    // Special case for Optimism chain
    if chain_config.chain_id == NamedChain::Optimism as ChainId {
        res.berlin_block = Some(3950000);
        res.london_block = Some(105235063);
        res.arrow_glacier_block = Some(105235063);
        res.gray_glacier_block = Some(105235063);
        res.merge_netsplit_block = Some(105235063);
    }

    // Add extra fields for ChainConfig from Genesis
    let extra_fields = ChainConfigExtraFields {
        bedrock_block: if chain_config.chain_id == NamedChain::Optimism as ChainId {
            Some(105235063)
        } else {
            Some(0)
        },
        regolith_time: Some(0),
        canyon_time: chain_config.hardforks.canyon_time,
        delta_time: chain_config.hardforks.delta_time,
        ecotone_time: chain_config.hardforks.ecotone_time,
        fjord_time: chain_config.hardforks.fjord_time,
        granite_time: chain_config.hardforks.granite_time,
        holocene_time: chain_config.hardforks.holocene_time,
        isthmus_time: chain_config.hardforks.isthmus_time,
        optimism: chain_config.optimism.as_ref().map(|o| o.into()),
    };
    res.extra_fields =
        serde_json::to_value(extra_fields).unwrap_or_default().try_into().unwrap_or_default();

    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};

    const BASE_CHAIN_METADATA: &str = r#"
    {
      "name": "Base",
      "public_rpc": "https://mainnet.base.org",
      "sequencer_rpc": "https://mainnet-sequencer.base.org",
      "explorer": "https://explorer.base.org",
      "superchain_level": 1,
      "governed_by_optimism": false,
      "superchain_time": 0,
      "data_availability_type": "eth-da",
      "chain_id": 8453,
      "batch_inbox_addr": "0xFf00000000000000000000000000000000008453",
      "block_time": 2,
      "seq_window_size": 3600,
      "max_sequencer_drift": 600,
      "hardforks": {
        "canyon_time": 1704992401,
        "delta_time": 1708560000,
        "ecotone_time": 1710374401,
        "fjord_time": 1720627201,
        "granite_time": 1726070401,
        "holocene_time": 1736445601
      },
      "optimism": {
        "eip1559_elasticity": 6,
        "eip1559_denominator": 50,
        "eip1559_denominator_canyon": 250
      },
      "genesis": {
        "l2_time": 1686789347,
        "l1": {
          "hash": "0x5c13d307623a926cd31415036c8b7fa14572f9dac64528e857a470511fc30771",
          "number": 17481768
        },
        "l2": {
          "hash": "0xf712aa9241cc24369b143cf6dce85f0902a9731e70d66818a3a5845b296c73dd",
          "number": 0
        },
        "system_config": {
          "batcherAddress": "0x5050F69a9786F081509234F1a7F4684b5E5b76C9",
          "overhead": "0x00000000000000000000000000000000000000000000000000000000000000bc",
          "scalar": "0x00000000000000000000000000000000000000000000000000000000000a6fe0",
          "gasLimit": 30000000
        }
      },
      "roles": {
        "SystemConfigOwner": "0x14536667Cd30e52C0b458BaACcB9faDA7046E056",
        "ProxyAdminOwner": "0x7bB41C3008B3f03FE483B28b8DB90e19Cf07595c",
        "Guardian": "0x09f7150D8c019BeF34450d6920f6B3608ceFdAf2",
        "Challenger": "0x6F8C5bA3F59ea3E76300E3BEcDC231D656017824",
        "Proposer": "0x642229f238fb9dE03374Be34B0eD8D9De80752c5",
        "UnsafeBlockSigner": "0xAf6E19BE0F9cE7f8afd49a1824851023A8249e8a",
        "BatchSubmitter": "0x5050F69a9786F081509234F1a7F4684b5E5b76C9"
      },
      "addresses": {
        "AddressManager": "0x8EfB6B5c4767B09Dc9AA6Af4eAA89F749522BaE2",
        "L1CrossDomainMessengerProxy": "0x866E82a600A1414e583f7F13623F1aC5d58b0Afa",
        "L1ERC721BridgeProxy": "0x608d94945A64503E642E6370Ec598e519a2C1E53",
        "L1StandardBridgeProxy": "0x3154Cf16ccdb4C6d922629664174b904d80F2C35",
        "L2OutputOracleProxy": "0x56315b90c40730925ec5485cf004d835058518A0",
        "OptimismMintableERC20FactoryProxy": "0x05cc379EBD9B30BbA19C6fA282AB29218EC61D84",
        "OptimismPortalProxy": "0x49048044D57e1C92A77f79988d21Fa8fAF74E97e",
        "SystemConfigProxy": "0x73a79Fab69143498Ed3712e519A88a918e1f4072",
        "ProxyAdmin": "0x0475cBCAebd9CE8AfA5025828d5b98DFb67E059E",
        "AnchorStateRegistryProxy": "0xdB9091e48B1C42992A1213e6916184f9eBDbfEDf",
        "DelayedWETHProxy": "0xa2f2aC6F5aF72e494A227d79Db20473Cf7A1FFE8",
        "DisputeGameFactoryProxy": "0x43edB88C4B80fDD2AdFF2412A7BebF9dF42cB40e",
        "FaultDisputeGame": "0xCd3c0194db74C23807D4B90A5181e1B28cF7007C",
        "MIPS": "0x16e83cE5Ce29BF90AD9Da06D2fE6a15d5f344ce4",
        "PermissionedDisputeGame": "0x19009dEBF8954B610f207D5925EEDe827805986e",
        "PreimageOracle": "0x9c065e11870B891D214Bc2Da7EF1f9DDFA1BE277"
      }
    }
    "#;

    #[test]
    fn test_deserialize_chain_config() {
        let config: ChainMetadata = serde_json::from_str(BASE_CHAIN_METADATA).unwrap();
        assert_eq!(config.name, "Base");
        assert_eq!(config.public_rpc, "https://mainnet.base.org");
        assert_eq!(config.sequencer_rpc, "https://mainnet-sequencer.base.org");
        assert_eq!(config.explorer, "https://explorer.base.org");
        assert_eq!(config.superchain_level, 1);
        assert!(!config.governed_by_optimism);
        assert_eq!(config.superchain_time, Some(0));
        assert_eq!(config.data_availability_type, "eth-da");
        assert_eq!(config.chain_id, 8453);
        assert_eq!(config.batch_inbox_addr, address!("Ff00000000000000000000000000000000008453"));
        assert_eq!(config.block_time, 2);
        assert_eq!(config.seq_window_size, 3600);
        assert_eq!(config.max_sequencer_drift, 600);
        // hardforks
        assert_eq!(config.hardforks.canyon_time, Some(1704992401));
        assert_eq!(config.hardforks.delta_time, Some(1708560000));
        assert_eq!(config.hardforks.ecotone_time, Some(1710374401));
        assert_eq!(config.hardforks.fjord_time, Some(1720627201));
        assert_eq!(config.hardforks.granite_time, Some(1726070401));
        assert_eq!(config.hardforks.holocene_time, Some(1736445601));
        // optimism
        assert_eq!(config.optimism.as_ref().unwrap().eip1559_elasticity, 6);
        assert_eq!(config.optimism.as_ref().unwrap().eip1559_denominator, 50);
        assert_eq!(config.optimism.as_ref().unwrap().eip1559_denominator_canyon, Some(250));
        // genesis
        assert_eq!(config.genesis.l2_time, 1686789347);
        assert_eq!(
            config.genesis.l1.hash,
            b256!("5c13d307623a926cd31415036c8b7fa14572f9dac64528e857a470511fc30771")
        );
        assert_eq!(config.genesis.l1.number, 17481768);
        assert_eq!(
            config.genesis.l2.hash,
            b256!("f712aa9241cc24369b143cf6dce85f0902a9731e70d66818a3a5845b296c73dd")
        );
        assert_eq!(config.genesis.l2.number, 0);
        assert_eq!(
            config.genesis.system_config.batcher_address,
            address!("5050F69a9786F081509234F1a7F4684b5E5b76C9")
        );
        assert_eq!(
            config.genesis.system_config.overhead,
            b256!("00000000000000000000000000000000000000000000000000000000000000bc")
        );
        assert_eq!(
            config.genesis.system_config.scalar,
            b256!("00000000000000000000000000000000000000000000000000000000000a6fe0")
        );
        assert_eq!(config.genesis.system_config.gas_limit, 30000000);
        // roles
        assert_eq!(
            config.roles.system_config_owner,
            Some(address!("14536667Cd30e52C0b458BaACcB9faDA7046E056"))
        );
        assert_eq!(
            config.roles.proxy_admin_owner,
            Some(address!("7bB41C3008B3f03FE483B28b8DB90e19Cf07595c"))
        );
        assert_eq!(
            config.roles.guardian,
            Some(address!("09f7150D8c019BeF34450d6920f6B3608ceFdAf2"))
        );
        assert_eq!(
            config.roles.challenger,
            Some(address!("6F8C5bA3F59ea3E76300E3BEcDC231D656017824"))
        );
        assert_eq!(
            config.roles.proposer,
            Some(address!("642229f238fb9dE03374Be34B0eD8D9De80752c5"))
        );
        assert_eq!(
            config.roles.unsafe_block_signer,
            Some(address!("Af6E19BE0F9cE7f8afd49a1824851023A8249e8a"))
        );
        assert_eq!(
            config.roles.batch_submitter,
            Some(address!("5050F69a9786F081509234F1a7F4684b5E5b76C9"))
        );
        // addresses
        assert_eq!(
            config.addresses.address_manager,
            Some(address!("8EfB6B5c4767B09Dc9AA6Af4eAA89F749522BaE2"))
        );
        assert_eq!(
            config.addresses.l1_cross_domain_messenger_proxy,
            Some(address!("866E82a600A1414e583f7F13623F1aC5d58b0Afa"))
        );
        assert_eq!(
            config.addresses.l1_erc721_bridge_proxy,
            Some(address!("608d94945A64503E642E6370Ec598e519a2C1E53"))
        );
        assert_eq!(
            config.addresses.l1_standard_bridge_proxy,
            Some(address!("3154Cf16ccdb4C6d922629664174b904d80F2C35"))
        );
        assert_eq!(
            config.addresses.l2_output_oracle_proxy,
            Some(address!("56315b90c40730925ec5485cf004d835058518A0"))
        );
        assert_eq!(
            config.addresses.optimism_mintable_erc20_factory_proxy,
            Some(address!("05cc379EBD9B30BbA19C6fA282AB29218EC61D84"))
        );
        assert_eq!(
            config.addresses.optimism_portal_proxy,
            Some(address!("49048044D57e1C92A77f79988d21Fa8fAF74E97e"))
        );
        assert_eq!(
            config.addresses.system_config_proxy,
            Some(address!("73a79Fab69143498Ed3712e519A88a918e1f4072"))
        );
        assert_eq!(
            config.addresses.proxy_admin,
            Some(address!("0475cBCAebd9CE8AfA5025828d5b98DFb67E059E"))
        );
        assert_eq!(
            config.addresses.anchor_state_registry_proxy,
            Some(address!("dB9091e48B1C42992A1213e6916184f9eBDbfEDf"))
        );
        assert_eq!(
            config.addresses.delayed_weth_proxy,
            Some(address!("a2f2aC6F5aF72e494A227d79Db20473Cf7A1FFE8"))
        );
        assert_eq!(
            config.addresses.dispute_game_factory_proxy,
            Some(address!("43edB88C4B80fDD2AdFF2412A7BebF9dF42cB40e"))
        );
        assert_eq!(
            config.addresses.fault_dispute_game,
            Some(address!("Cd3c0194db74C23807D4B90A5181e1B28cF7007C"))
        );
        assert_eq!(
            config.addresses.mips,
            Some(address!("16e83cE5Ce29BF90AD9Da06D2fE6a15d5f344ce4"))
        );
        assert_eq!(
            config.addresses.permissioned_dispute_game,
            Some(address!("19009dEBF8954B610f207D5925EEDe827805986e"))
        );
        assert_eq!(
            config.addresses.preimage_oracle,
            Some(address!("9c065e11870B891D214Bc2Da7EF1f9DDFA1BE277"))
        );
    }

    #[test]
    fn test_chain_config_extra_fields() {
        let extra_fields = ChainConfigExtraFields {
            bedrock_block: Some(105235063),
            regolith_time: Some(0),
            canyon_time: Some(1704992401),
            delta_time: Some(1708560000),
            ecotone_time: Some(1710374401),
            fjord_time: Some(1720627201),
            granite_time: Some(1726070401),
            holocene_time: Some(1736445601),
            isthmus_time: None,
            optimism: Option::from(ChainConfigExtraFieldsOptimism {
                eip1559_elasticity: 6,
                eip1559_denominator: 50,
                eip1559_denominator_canyon: Some(250),
            }),
        };
        let value = serde_json::to_value(extra_fields).unwrap();
        assert_eq!(value.get("bedrockBlock").unwrap(), 105235063);
        assert_eq!(value.get("regolithTime").unwrap(), 0);
        assert_eq!(value.get("canyonTime").unwrap(), 1704992401);
        assert_eq!(value.get("deltaTime").unwrap(), 1708560000);
        assert_eq!(value.get("ecotoneTime").unwrap(), 1710374401);
        assert_eq!(value.get("fjordTime").unwrap(), 1720627201);
        assert_eq!(value.get("graniteTime").unwrap(), 1726070401);
        assert_eq!(value.get("holoceneTime").unwrap(), 1736445601);
        assert_eq!(value.get("isthmusTime"), None);
        let optimism = value.get("optimism").unwrap();
        assert_eq!(optimism.get("eip1559Elasticity").unwrap(), 6);
        assert_eq!(optimism.get("eip1559Denominator").unwrap(), 50);
        assert_eq!(optimism.get("eip1559DenominatorCanyon").unwrap(), 250);
    }

    #[test]
    fn test_convert_to_genesis_chain_config() {
        let config: ChainMetadata = serde_json::from_str(BASE_CHAIN_METADATA).unwrap();
        let chain_config = to_genesis_chain_config(&config);
        assert_eq!(chain_config.chain_id, 8453);
        assert_eq!(chain_config.homestead_block, Some(0));
        assert_eq!(chain_config.dao_fork_block, None);
        assert!(!chain_config.dao_fork_support);
        assert_eq!(chain_config.eip150_block, Some(0));
        assert_eq!(chain_config.eip155_block, Some(0));
        assert_eq!(chain_config.eip158_block, Some(0));
        assert_eq!(chain_config.byzantium_block, Some(0));
        assert_eq!(chain_config.constantinople_block, Some(0));
        assert_eq!(chain_config.petersburg_block, Some(0));
        assert_eq!(chain_config.istanbul_block, Some(0));
        assert_eq!(chain_config.muir_glacier_block, Some(0));
        assert_eq!(chain_config.berlin_block, Some(0));
        assert_eq!(chain_config.london_block, Some(0));
        assert_eq!(chain_config.arrow_glacier_block, Some(0));
        assert_eq!(chain_config.gray_glacier_block, Some(0));
        assert_eq!(chain_config.merge_netsplit_block, Some(0));
        assert_eq!(chain_config.shanghai_time, Some(1704992401));
        assert_eq!(chain_config.cancun_time, Some(1710374401));
        assert_eq!(chain_config.prague_time, None);
        assert_eq!(chain_config.osaka_time, None);
        assert_eq!(chain_config.terminal_total_difficulty, Some(U256::ZERO));
        assert!(chain_config.terminal_total_difficulty_passed);
        assert_eq!(chain_config.ethash, None);
        assert_eq!(chain_config.clique, None);
        assert_eq!(chain_config.extra_fields.get("bedrockBlock").unwrap(), 0);
        assert_eq!(chain_config.extra_fields.get("regolithTime").unwrap(), 0);
        assert_eq!(chain_config.extra_fields.get("canyonTime").unwrap(), 1704992401);
        assert_eq!(chain_config.extra_fields.get("deltaTime").unwrap(), 1708560000);
        assert_eq!(chain_config.extra_fields.get("ecotoneTime").unwrap(), 1710374401);
        assert_eq!(chain_config.extra_fields.get("fjordTime").unwrap(), 1720627201);
        assert_eq!(chain_config.extra_fields.get("graniteTime").unwrap(), 1726070401);
        assert_eq!(chain_config.extra_fields.get("holoceneTime").unwrap(), 1736445601);
        assert_eq!(chain_config.extra_fields.get("isthmusTime"), None);
        let optimism = chain_config.extra_fields.get("optimism").unwrap();
        assert_eq!(optimism.get("eip1559Elasticity").unwrap(), 6);
        assert_eq!(optimism.get("eip1559Denominator").unwrap(), 50);
        assert_eq!(optimism.get("eip1559DenominatorCanyon").unwrap(), 250);
    }

    #[test]
    fn test_convert_to_genesis_chain_config_op() {
        const OP_CHAIN_METADATA: &str = r#"
        {
          "name": "OP Mainnet",
          "public_rpc": "https://mainnet.optimism.io",
          "sequencer_rpc": "https://mainnet-sequencer.optimism.io",
          "explorer": "https://explorer.optimism.io",
          "superchain_level": 2,
          "governed_by_optimism": true,
          "superchain_time": 0,
          "data_availability_type": "eth-da",
          "chain_id": 10,
          "batch_inbox_addr": "0xFF00000000000000000000000000000000000010",
          "block_time": 2,
          "seq_window_size": 3600,
          "max_sequencer_drift": 600,
          "hardforks": {
            "canyon_time": 1704992401,
            "delta_time": 1708560000,
            "ecotone_time": 1710374401,
            "fjord_time": 1720627201,
            "granite_time": 1726070401,
            "holocene_time": 1736445601
          },
          "optimism": {
            "eip1559_elasticity": 6,
            "eip1559_denominator": 50,
            "eip1559_denominator_canyon": 250
          },
          "genesis": {
            "l2_time": 1686068903,
            "l1": {
              "hash": "0x438335a20d98863a4c0c97999eb2481921ccd28553eac6f913af7c12aec04108",
              "number": 17422590
            },
            "l2": {
              "hash": "0xdbf6a80fef073de06add9b0d14026d6e5a86c85f6d102c36d3d8e9cf89c2afd3",
              "number": 105235063
            },
            "system_config": {
              "batcherAddress": "0x6887246668a3b87F54DeB3b94Ba47a6f63F32985",
              "overhead": "0x00000000000000000000000000000000000000000000000000000000000000bc",
              "scalar": "0x00000000000000000000000000000000000000000000000000000000000a6fe0",
              "gasLimit": 30000000
            }
          },
          "roles": {
            "SystemConfigOwner": "0x847B5c174615B1B7fDF770882256e2D3E95b9D92",
            "ProxyAdminOwner": "0x5a0Aae59D09fccBdDb6C6CcEB07B7279367C3d2A",
            "Guardian": "0x09f7150D8c019BeF34450d6920f6B3608ceFdAf2",
            "Challenger": "0x9BA6e03D8B90dE867373Db8cF1A58d2F7F006b3A",
            "Proposer": "0x473300df21D047806A082244b417f96b32f13A33",
            "UnsafeBlockSigner": "0xAAAA45d9549EDA09E70937013520214382Ffc4A2",
            "BatchSubmitter": "0x6887246668a3b87F54DeB3b94Ba47a6f63F32985"
          },
          "addresses": {
            "AddressManager": "0xdE1FCfB0851916CA5101820A69b13a4E276bd81F",
            "L1CrossDomainMessengerProxy": "0x25ace71c97B33Cc4729CF772ae268934F7ab5fA1",
            "L1ERC721BridgeProxy": "0x5a7749f83b81B301cAb5f48EB8516B986DAef23D",
            "L1StandardBridgeProxy": "0x99C9fc46f92E8a1c0deC1b1747d010903E884bE1",
            "OptimismMintableERC20FactoryProxy": "0x75505a97BD334E7BD3C476893285569C4136Fa0F",
            "OptimismPortalProxy": "0xbEb5Fc579115071764c7423A4f12eDde41f106Ed",
            "SystemConfigProxy": "0x229047fed2591dbec1eF1118d64F7aF3dB9EB290",
            "ProxyAdmin": "0x543bA4AADBAb8f9025686Bd03993043599c6fB04",
            "AnchorStateRegistryProxy": "0x18DAc71c228D1C32c99489B7323d441E1175e443",
            "DelayedWETHProxy": "0x82511d494B5C942BE57498a70Fdd7184Ee33B975",
            "DisputeGameFactoryProxy": "0xe5965Ab5962eDc7477C8520243A95517CD252fA9",
            "FaultDisputeGame": "0xA6f3DFdbf4855a43c529bc42EDE96797252879af",
            "MIPS": "0x16e83cE5Ce29BF90AD9Da06D2fE6a15d5f344ce4",
            "PermissionedDisputeGame": "0x050ed6F6273c7D836a111E42153BC00D0380b87d",
            "PreimageOracle": "0x9c065e11870B891D214Bc2Da7EF1f9DDFA1BE277"
          }
        }
        "#;
        let config: ChainMetadata = serde_json::from_str(OP_CHAIN_METADATA).unwrap();
        assert_eq!(config.name, "OP Mainnet");
        assert_eq!(config.hardforks.canyon_time, Some(1704992401));
        let chain_config = to_genesis_chain_config(&config);
        assert_eq!(chain_config.chain_id, 10);
        assert_eq!(chain_config.shanghai_time, Some(1704992401));
        assert_eq!(chain_config.cancun_time, Some(1710374401));
        assert_eq!(chain_config.prague_time, None);
        assert_eq!(chain_config.berlin_block, Some(3950000));
        assert_eq!(chain_config.london_block, Some(105235063));
        assert_eq!(chain_config.arrow_glacier_block, Some(105235063));
        assert_eq!(chain_config.gray_glacier_block, Some(105235063));
        assert_eq!(chain_config.merge_netsplit_block, Some(105235063));
        assert_eq!(chain_config.extra_fields.get("bedrockBlock").unwrap(), 105235063);
        assert_eq!(chain_config.extra_fields.get("regolithTime").unwrap(), 0);
        assert_eq!(chain_config.extra_fields.get("canyonTime").unwrap(), 1704992401);
        assert_eq!(chain_config.extra_fields.get("deltaTime").unwrap(), 1708560000);
        assert_eq!(chain_config.extra_fields.get("ecotoneTime").unwrap(), 1710374401);
        assert_eq!(chain_config.extra_fields.get("fjordTime").unwrap(), 1720627201);
        assert_eq!(chain_config.extra_fields.get("graniteTime").unwrap(), 1726070401);
        assert_eq!(chain_config.extra_fields.get("holoceneTime").unwrap(), 1736445601);
        assert_eq!(chain_config.extra_fields.get("isthmusTime"), None);
        let optimism = chain_config.extra_fields.get("optimism").unwrap();
        assert_eq!(optimism.get("eip1559Elasticity").unwrap(), 6);
        assert_eq!(optimism.get("eip1559Denominator").unwrap(), 50);
        assert_eq!(optimism.get("eip1559DenominatorCanyon").unwrap(), 250);
    }
}
