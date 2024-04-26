/// Optimism-specific genesis fields.
use alloy_genesis::Genesis;
use reth_primitives::{ChainSpec, ForkCondition, Hardfork};
use serde::Deserialize;

/// Genesis type for Optimism networks.
#[derive(Default, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OptimismGenesis {
    /// Wraps Eth genesis fields.
    #[serde(flatten)]
    pub eth_genesis: Genesis,
}

impl From<OptimismGenesis> for ChainSpec {
    fn from(optimsim_genesis: OptimismGenesis) -> ChainSpec {
        let mut chain_spec: ChainSpec = optimsim_genesis.eth_genesis.clone().into();

        if let Some(block) = optimsim_genesis.eth_genesis.config.extra_fields.get("bedrock_block") {
            chain_spec
                .hardforks
                .insert(Hardfork::Bedrock, ForkCondition::Block(block.as_u64().unwrap()));
        }
        if let Some(timestamp) =
            optimsim_genesis.eth_genesis.config.extra_fields.get("regolith_timestamp")
        {
            chain_spec
                .hardforks
                .insert(Hardfork::Regolith, ForkCondition::Timestamp(timestamp.as_u64().unwrap()));
        }
        if let Some(timestamp) =
            optimsim_genesis.eth_genesis.config.extra_fields.get("ecotone_timestamp")
        {
            chain_spec
                .hardforks
                .insert(Hardfork::Ecotone, ForkCondition::Timestamp(timestamp.as_u64().unwrap()));
        }
        if let Some(timestamp) =
            optimsim_genesis.eth_genesis.config.extra_fields.get("canyon_timestamp")
        {
            chain_spec
                .hardforks
                .insert(Hardfork::Canyon, ForkCondition::Timestamp(timestamp.as_u64().unwrap()));
        }

        chain_spec
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::ChainConfig;
    use serde_json::Value;
    use std::collections::BTreeMap;

    #[test]
    fn parse_genesis() {
        let genesis = r#"
    {
      "nonce": 9,
      "config": {
        "chainId": 1,
        "bedrockBlock": 10,
        "regolithTime": 20,
        "ecotoneTime": 30,
        "canyonTime": 40,
        "optimism": {
          "eip1559Elasticity": 50,
          "eip1559Denominator": 60,
          "eip1559DenominatorCanyon": 70
        }
      }
    }
    "#;
        let optimism_genesis: OptimismGenesis = serde_json::from_str(genesis).unwrap();

        let actual_nonce = optimism_genesis.eth_genesis.nonce;
        assert_eq!(actual_nonce, 9);
        let actual_chain_id = optimism_genesis.eth_genesis.config.chain_id;
        assert_eq!(actual_chain_id, 1);

        let actual_bedrock_block =
            optimism_genesis.eth_genesis.config.extra_fields.get("bedrock_block");
        assert_eq!(actual_bedrock_block, Some(Value::from(10)).as_ref());
        let actual_regolith_timestamp =
            optimism_genesis.eth_genesis.config.extra_fields.get("regolith_timestamp");
        assert_eq!(actual_regolith_timestamp, Some(Value::from(20)).as_ref());
        let actual_ecotone_timestamp =
            optimism_genesis.eth_genesis.config.extra_fields.get("ecotone_timestamp");
        assert_eq!(actual_ecotone_timestamp, Some(Value::from(30)).as_ref());
        let actual_canyon_timestamp =
            optimism_genesis.eth_genesis.config.extra_fields.get("canyon_timestamp");
        assert_eq!(actual_canyon_timestamp, Some(Value::from(40)).as_ref());

        let optimism_object =
            optimism_genesis.eth_genesis.config.extra_fields.get("optimism").unwrap();
        assert_eq!(
            optimism_object,
            &serde_json::json!({
                "eip1559Elasticity": 50,
                "eip1559Denominator": 60,
                "eip1559DenominatorCanyon": 70
            })
        );
    }

    #[test]
    fn optimism_genesis_into_chainspec() {
        let mut extra_fields: BTreeMap<String, Value> = BTreeMap::new();
        extra_fields
            .insert("bedrock_block".to_string(), Value::Number(serde_json::Number::from(1)));
        extra_fields
            .insert("regolith_timestamp".to_string(), Value::Number(serde_json::Number::from(2)));
        extra_fields
            .insert("ecotone_timestamp".to_string(), Value::Number(serde_json::Number::from(3)));
        extra_fields
            .insert("canyon_timestamp".to_string(), Value::Number(serde_json::Number::from(4)));
        let optimism_genesis = OptimismGenesis {
            eth_genesis: Genesis {
                config: ChainConfig { extra_fields, ..Default::default() },
                ..Default::default()
            },
        };

        let chain_spec: ChainSpec = optimism_genesis.into();

        assert!(!chain_spec.is_fork_active_at_block(Hardfork::Bedrock, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(Hardfork::Regolith, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(Hardfork::Ecotone, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(Hardfork::Canyon, 0));

        assert!(chain_spec.is_fork_active_at_block(Hardfork::Bedrock, 1));
        assert!(chain_spec.is_fork_active_at_timestamp(Hardfork::Regolith, 2));
        assert!(chain_spec.is_fork_active_at_timestamp(Hardfork::Ecotone, 3));
        assert!(chain_spec.is_fork_active_at_timestamp(Hardfork::Canyon, 4));
    }
}
