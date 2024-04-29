/// Optimism-specific genesis fields.
use reth_primitives::{AllGenesisFormats, ChainSpec, ForkCondition, Hardfork};
use serde::Deserialize;

const BEDROCK_BLOCK_FIELD_NAME: &str = "bedrockBlock";
const REGOLITH_TIME_FIELD_NAME: &str = "regolithTime";
const ECOTONE_TIME_FIELD_NAME: &str = "ecotoneTime";
const CANYON_TIME_FIELD_NAME: &str = "canyonTime";

/// Genesis type for Optimism networks.
#[derive(Debug, Deserialize)]
pub(crate) struct OptimismGenesis(AllGenesisFormats);

impl From<OptimismGenesis> for ChainSpec {
    fn from(optimsim_genesis: OptimismGenesis) -> ChainSpec {
        match optimsim_genesis.0 {
            AllGenesisFormats::Reth(chain_spec) => chain_spec,
            AllGenesisFormats::Geth(genesis) => {
                let mut chain_spec: ChainSpec = genesis.clone().into();
                if let Some(block) = genesis.config.extra_fields.get(BEDROCK_BLOCK_FIELD_NAME) {
                    chain_spec.hardforks.insert(
                        Hardfork::Bedrock,
                        ForkCondition::Block(block.as_u64().unwrap_or_else(|| {
                            panic!("{BEDROCK_BLOCK_FIELD_NAME} should be a number")
                        })),
                    );
                }
                if let Some(timestamp) = genesis.config.extra_fields.get(REGOLITH_TIME_FIELD_NAME) {
                    chain_spec.hardforks.insert(
                        Hardfork::Regolith,
                        ForkCondition::Timestamp(timestamp.as_u64().unwrap_or_else(|| {
                            panic!("{REGOLITH_TIME_FIELD_NAME} should be a number")
                        })),
                    );
                }
                if let Some(timestamp) = genesis.config.extra_fields.get(ECOTONE_TIME_FIELD_NAME) {
                    chain_spec.hardforks.insert(
                        Hardfork::Ecotone,
                        ForkCondition::Timestamp(timestamp.as_u64().unwrap_or_else(|| {
                            panic!("{ECOTONE_TIME_FIELD_NAME} should be a number")
                        })),
                    );
                }
                if let Some(timestamp) = genesis.config.extra_fields.get(CANYON_TIME_FIELD_NAME) {
                    chain_spec.hardforks.insert(
                        Hardfork::Canyon,
                        ForkCondition::Timestamp(timestamp.as_u64().unwrap_or_else(|| {
                            panic!("{CANYON_TIME_FIELD_NAME} should be a number")
                        })),
                    );
                }
                chain_spec
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_genesis::Genesis;
    use reth_primitives::ChainConfig;
    use serde_json::Value;
    use std::collections::BTreeMap;

    #[test]
    fn parse_genesis() {
        let genesis = format!(
            "
    {{
      \"nonce\": 9,
      \"config\": {{
        \"chainId\": 1,
        \"{bedrockBlock}\": 10,
        \"{regolithTime}\": 20,
        \"{ecotoneTime}\": 30,
        \"{canyonTime}\": 40,
        \"optimism\": {{
          \"eip1559Elasticity\": 50,
          \"eip1559Denominator\": 60,
          \"eip1559DenominatorCanyon\": 70
        }}
      }}
    }}
    ",
            bedrockBlock = BEDROCK_BLOCK_FIELD_NAME,
            regolithTime = REGOLITH_TIME_FIELD_NAME,
            ecotoneTime = ECOTONE_TIME_FIELD_NAME,
            canyonTime = CANYON_TIME_FIELD_NAME,
        );
        let optimism_genesis: OptimismGenesis = serde_json::from_str(&genesis).unwrap();

        if let AllGenesisFormats::Geth(genesis) = optimism_genesis.0 {
            let actual_nonce = genesis.nonce;
            assert_eq!(actual_nonce, 9);
            let actual_chain_id = genesis.config.chain_id;
            assert_eq!(actual_chain_id, 1);

            let actual_bedrock_block = genesis.config.extra_fields.get(BEDROCK_BLOCK_FIELD_NAME);
            assert_eq!(actual_bedrock_block, Some(Value::from(10)).as_ref());
            let actual_regolith_timestamp =
                genesis.config.extra_fields.get(REGOLITH_TIME_FIELD_NAME);
            assert_eq!(actual_regolith_timestamp, Some(Value::from(20)).as_ref());
            let actual_ecotone_timestamp = genesis.config.extra_fields.get(ECOTONE_TIME_FIELD_NAME);
            assert_eq!(actual_ecotone_timestamp, Some(Value::from(30)).as_ref());
            let actual_canyon_timestamp = genesis.config.extra_fields.get(CANYON_TIME_FIELD_NAME);
            assert_eq!(actual_canyon_timestamp, Some(Value::from(40)).as_ref());

            let optimism_object = genesis.config.extra_fields.get("optimism").unwrap();
            assert_eq!(
                optimism_object,
                &serde_json::json!({
                    "eip1559Elasticity": 50,
                    "eip1559Denominator": 60,
                    "eip1559DenominatorCanyon": 70
                })
            );
        } else {
            panic!("unexpected OptimismGenesis variant");
        }
    }

    #[test]
    fn optimism_genesis_into_chainspec() {
        let mut extra_fields: BTreeMap<String, Value> = BTreeMap::new();
        extra_fields.insert(
            BEDROCK_BLOCK_FIELD_NAME.to_string(),
            Value::Number(serde_json::Number::from(1)),
        );
        extra_fields.insert(
            REGOLITH_TIME_FIELD_NAME.to_string(),
            Value::Number(serde_json::Number::from(2)),
        );
        extra_fields.insert(
            ECOTONE_TIME_FIELD_NAME.to_string(),
            Value::Number(serde_json::Number::from(3)),
        );
        extra_fields
            .insert(CANYON_TIME_FIELD_NAME.to_string(), Value::Number(serde_json::Number::from(4)));
        let optimism_genesis = OptimismGenesis(AllGenesisFormats::Geth(Genesis {
            config: ChainConfig { extra_fields, ..Default::default() },
            ..Default::default()
        }));

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
