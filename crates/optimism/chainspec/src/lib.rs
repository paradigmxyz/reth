//! OP-Reth chain specs.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

extern crate alloc;

pub mod constants;

mod base;
mod base_sepolia;
mod dev;
mod op;
mod op_sepolia;

pub use base::BASE_MAINNET;
pub use base_sepolia::BASE_SEPOLIA;
pub use dev::OP_DEV;
pub use op::OP_MAINNET;
pub use op_sepolia::OP_SEPOLIA;

use derive_more::{Constructor, Deref, Into};
use reth_chainspec::ChainSpec;

/// OP stack chain spec type.
#[derive(Debug, Clone, Deref, Into, Constructor)]
pub struct OpChainSpec {
    /// [`ChainSpec`].
    pub inner: ChainSpec,
}

#[cfg(test)]
mod tests {
    use alloy_genesis::Genesis;
    use alloy_primitives::b256;
    use reth_chainspec::{test_fork_ids, BaseFeeParams, BaseFeeParamsKind, ChainSpec};
    use reth_ethereum_forks::{
        EthereumHardfork, ForkCondition, ForkHash, ForkId, Head, OptimismHardfork,
        OptimismHardforks,
    };

    use crate::*;

    #[test]
    fn base_mainnet_forkids() {
        test_fork_ids(
            &BASE_MAINNET,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x67, 0xda, 0x02, 0x60]), next: 1704992401 },
                ),
                (
                    Head { number: 0, timestamp: 1704992400, ..Default::default() },
                    ForkId { hash: ForkHash([0x67, 0xda, 0x02, 0x60]), next: 1704992401 },
                ),
                (
                    Head { number: 0, timestamp: 1704992401, ..Default::default() },
                    ForkId { hash: ForkHash([0x3c, 0x28, 0x3c, 0xb3]), next: 1710374401 },
                ),
                (
                    Head { number: 0, timestamp: 1710374400, ..Default::default() },
                    ForkId { hash: ForkHash([0x3c, 0x28, 0x3c, 0xb3]), next: 1710374401 },
                ),
                (
                    Head { number: 0, timestamp: 1710374401, ..Default::default() },
                    ForkId { hash: ForkHash([0x51, 0xcc, 0x98, 0xb3]), next: 1720627201 },
                ),
                (
                    Head { number: 0, timestamp: 1720627200, ..Default::default() },
                    ForkId { hash: ForkHash([0x51, 0xcc, 0x98, 0xb3]), next: 1720627201 },
                ),
                (
                    Head { number: 0, timestamp: 1720627201, ..Default::default() },
                    ForkId { hash: ForkHash([0xe4, 0x01, 0x0e, 0xb9]), next: 1726070401 },
                ),
                (
                    Head { number: 0, timestamp: 1726070401, ..Default::default() },
                    ForkId { hash: ForkHash([0xbc, 0x38, 0xf9, 0xca]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn op_sepolia_forkids() {
        test_fork_ids(
            &OP_SEPOLIA,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x67, 0xa4, 0x03, 0x28]), next: 1699981200 },
                ),
                (
                    Head { number: 0, timestamp: 1699981199, ..Default::default() },
                    ForkId { hash: ForkHash([0x67, 0xa4, 0x03, 0x28]), next: 1699981200 },
                ),
                (
                    Head { number: 0, timestamp: 1699981200, ..Default::default() },
                    ForkId { hash: ForkHash([0xa4, 0x8d, 0x6a, 0x00]), next: 1708534800 },
                ),
                (
                    Head { number: 0, timestamp: 1708534799, ..Default::default() },
                    ForkId { hash: ForkHash([0xa4, 0x8d, 0x6a, 0x00]), next: 1708534800 },
                ),
                (
                    Head { number: 0, timestamp: 1708534800, ..Default::default() },
                    ForkId { hash: ForkHash([0xcc, 0x17, 0xc7, 0xeb]), next: 1716998400 },
                ),
                (
                    Head { number: 0, timestamp: 1716998399, ..Default::default() },
                    ForkId { hash: ForkHash([0xcc, 0x17, 0xc7, 0xeb]), next: 1716998400 },
                ),
                (
                    Head { number: 0, timestamp: 1716998400, ..Default::default() },
                    ForkId { hash: ForkHash([0x54, 0x0a, 0x8c, 0x5d]), next: 1723478400 },
                ),
                (
                    Head { number: 0, timestamp: 1723478399, ..Default::default() },
                    ForkId { hash: ForkHash([0x54, 0x0a, 0x8c, 0x5d]), next: 1723478400 },
                ),
                (
                    Head { number: 0, timestamp: 1723478400, ..Default::default() },
                    ForkId { hash: ForkHash([0x75, 0xde, 0xa4, 0x1e]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn op_mainnet_forkids() {
        test_fork_ids(
            &OP_MAINNET,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xca, 0xf5, 0x17, 0xed]), next: 3950000 },
                ),
                // TODO: complete these, see https://github.com/paradigmxyz/reth/issues/8012
                (
                    Head { number: 105235063, timestamp: 1710374401, ..Default::default() },
                    ForkId { hash: ForkHash([0x19, 0xda, 0x4c, 0x52]), next: 1720627201 },
                ),
            ],
        );
    }

    #[test]
    fn base_sepolia_forkids() {
        test_fork_ids(
            &BASE_SEPOLIA,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xb9, 0x59, 0xb9, 0xf7]), next: 1699981200 },
                ),
                (
                    Head { number: 0, timestamp: 1699981199, ..Default::default() },
                    ForkId { hash: ForkHash([0xb9, 0x59, 0xb9, 0xf7]), next: 1699981200 },
                ),
                (
                    Head { number: 0, timestamp: 1699981200, ..Default::default() },
                    ForkId { hash: ForkHash([0x60, 0x7c, 0xd5, 0xa1]), next: 1708534800 },
                ),
                (
                    Head { number: 0, timestamp: 1708534799, ..Default::default() },
                    ForkId { hash: ForkHash([0x60, 0x7c, 0xd5, 0xa1]), next: 1708534800 },
                ),
                (
                    Head { number: 0, timestamp: 1708534800, ..Default::default() },
                    ForkId { hash: ForkHash([0xbe, 0x96, 0x9b, 0x17]), next: 1716998400 },
                ),
                (
                    Head { number: 0, timestamp: 1716998399, ..Default::default() },
                    ForkId { hash: ForkHash([0xbe, 0x96, 0x9b, 0x17]), next: 1716998400 },
                ),
                (
                    Head { number: 0, timestamp: 1716998400, ..Default::default() },
                    ForkId { hash: ForkHash([0x4e, 0x45, 0x7a, 0x49]), next: 1723478400 },
                ),
                (
                    Head { number: 0, timestamp: 1723478399, ..Default::default() },
                    ForkId { hash: ForkHash([0x4e, 0x45, 0x7a, 0x49]), next: 1723478400 },
                ),
                (
                    Head { number: 0, timestamp: 1723478400, ..Default::default() },
                    ForkId { hash: ForkHash([0x5e, 0xdf, 0xa3, 0xb6]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn base_mainnet_genesis() {
        let genesis = BASE_MAINNET.genesis_header();
        assert_eq!(
            genesis.hash_slow(),
            b256!("f712aa9241cc24369b143cf6dce85f0902a9731e70d66818a3a5845b296c73dd")
        );
        let base_fee = genesis
            .next_block_base_fee(BASE_MAINNET.base_fee_params_at_timestamp(genesis.timestamp))
            .unwrap();
        // <https://base.blockscout.com/block/1>
        assert_eq!(base_fee, 980000000);
    }

    #[test]
    fn base_sepolia_genesis() {
        let genesis = BASE_SEPOLIA.genesis_header();
        assert_eq!(
            genesis.hash_slow(),
            b256!("0dcc9e089e30b90ddfc55be9a37dd15bc551aeee999d2e2b51414c54eaf934e4")
        );
        let base_fee = genesis
            .next_block_base_fee(BASE_SEPOLIA.base_fee_params_at_timestamp(genesis.timestamp))
            .unwrap();
        // <https://base-sepolia.blockscout.com/block/1>
        assert_eq!(base_fee, 980000000);
    }

    #[test]
    fn op_sepolia_genesis() {
        let genesis = OP_SEPOLIA.genesis_header();
        assert_eq!(
            genesis.hash_slow(),
            b256!("102de6ffb001480cc9b8b548fd05c34cd4f46ae4aa91759393db90ea0409887d")
        );
        let base_fee = genesis
            .next_block_base_fee(OP_SEPOLIA.base_fee_params_at_timestamp(genesis.timestamp))
            .unwrap();
        // <https://optimism-sepolia.blockscout.com/block/1>
        assert_eq!(base_fee, 980000000);
    }

    #[test]
    fn latest_base_mainnet_fork_id() {
        assert_eq!(
            ForkId { hash: ForkHash([0xbc, 0x38, 0xf9, 0xca]), next: 0 },
            BASE_MAINNET.latest_fork_id()
        )
    }

    #[test]
    fn is_bedrock_active() {
        assert!(!OP_MAINNET.is_bedrock_active_at_block(1))
    }

    #[test]
    fn parse_optimism_hardforks() {
        let geth_genesis = r#"
    {
      "config": {
        "bedrockBlock": 10,
        "regolithTime": 20,
        "canyonTime": 30,
        "ecotoneTime": 40,
        "fjordTime": 50,
        "graniteTime": 51,
        "optimism": {
          "eip1559Elasticity": 60,
          "eip1559Denominator": 70
        }
      }
    }
    "#;
        let genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();

        let actual_bedrock_block = genesis.config.extra_fields.get("bedrockBlock");
        assert_eq!(actual_bedrock_block, Some(serde_json::Value::from(10)).as_ref());
        let actual_regolith_timestamp = genesis.config.extra_fields.get("regolithTime");
        assert_eq!(actual_regolith_timestamp, Some(serde_json::Value::from(20)).as_ref());
        let actual_canyon_timestamp = genesis.config.extra_fields.get("canyonTime");
        assert_eq!(actual_canyon_timestamp, Some(serde_json::Value::from(30)).as_ref());
        let actual_ecotone_timestamp = genesis.config.extra_fields.get("ecotoneTime");
        assert_eq!(actual_ecotone_timestamp, Some(serde_json::Value::from(40)).as_ref());
        let actual_fjord_timestamp = genesis.config.extra_fields.get("fjordTime");
        assert_eq!(actual_fjord_timestamp, Some(serde_json::Value::from(50)).as_ref());
        let actual_granite_timestamp = genesis.config.extra_fields.get("graniteTime");
        assert_eq!(actual_granite_timestamp, Some(serde_json::Value::from(51)).as_ref());

        let optimism_object = genesis.config.extra_fields.get("optimism").unwrap();
        assert_eq!(
            optimism_object,
            &serde_json::json!({
                "eip1559Elasticity": 60,
                "eip1559Denominator": 70,
            })
        );

        let chain_spec: ChainSpec = genesis.into();

        assert_eq!(
            chain_spec.base_fee_params,
            BaseFeeParamsKind::Constant(BaseFeeParams::new(70, 60))
        );

        assert!(!chain_spec.is_fork_active_at_block(OptimismHardfork::Bedrock, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Regolith, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Canyon, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Ecotone, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Fjord, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Granite, 0));

        assert!(chain_spec.is_fork_active_at_block(OptimismHardfork::Bedrock, 10));
        assert!(chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Regolith, 20));
        assert!(chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Canyon, 30));
        assert!(chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Ecotone, 40));
        assert!(chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Fjord, 50));
        assert!(chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Granite, 51));
    }

    #[test]
    fn parse_optimism_hardforks_variable_base_fee_params() {
        let geth_genesis = r#"
    {
      "config": {
        "bedrockBlock": 10,
        "regolithTime": 20,
        "canyonTime": 30,
        "ecotoneTime": 40,
        "fjordTime": 50,
        "graniteTime": 51,
        "optimism": {
          "eip1559Elasticity": 60,
          "eip1559Denominator": 70,
          "eip1559DenominatorCanyon": 80
        }
      }
    }
    "#;
        let genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();

        let actual_bedrock_block = genesis.config.extra_fields.get("bedrockBlock");
        assert_eq!(actual_bedrock_block, Some(serde_json::Value::from(10)).as_ref());
        let actual_regolith_timestamp = genesis.config.extra_fields.get("regolithTime");
        assert_eq!(actual_regolith_timestamp, Some(serde_json::Value::from(20)).as_ref());
        let actual_canyon_timestamp = genesis.config.extra_fields.get("canyonTime");
        assert_eq!(actual_canyon_timestamp, Some(serde_json::Value::from(30)).as_ref());
        let actual_ecotone_timestamp = genesis.config.extra_fields.get("ecotoneTime");
        assert_eq!(actual_ecotone_timestamp, Some(serde_json::Value::from(40)).as_ref());
        let actual_fjord_timestamp = genesis.config.extra_fields.get("fjordTime");
        assert_eq!(actual_fjord_timestamp, Some(serde_json::Value::from(50)).as_ref());
        let actual_granite_timestamp = genesis.config.extra_fields.get("graniteTime");
        assert_eq!(actual_granite_timestamp, Some(serde_json::Value::from(51)).as_ref());

        let optimism_object = genesis.config.extra_fields.get("optimism").unwrap();
        assert_eq!(
            optimism_object,
            &serde_json::json!({
                "eip1559Elasticity": 60,
                "eip1559Denominator": 70,
                "eip1559DenominatorCanyon": 80
            })
        );

        let chain_spec: ChainSpec = genesis.into();

        assert_eq!(
            chain_spec.base_fee_params,
            BaseFeeParamsKind::Variable(
                vec![
                    (EthereumHardfork::London.boxed(), BaseFeeParams::new(70, 60)),
                    (OptimismHardfork::Canyon.boxed(), BaseFeeParams::new(80, 60)),
                ]
                .into()
            )
        );

        assert!(!chain_spec.is_fork_active_at_block(OptimismHardfork::Bedrock, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Regolith, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Canyon, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Ecotone, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Fjord, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Granite, 0));

        assert!(chain_spec.is_fork_active_at_block(OptimismHardfork::Bedrock, 10));
        assert!(chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Regolith, 20));
        assert!(chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Canyon, 30));
        assert!(chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Ecotone, 40));
        assert!(chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Fjord, 50));
        assert!(chain_spec.is_fork_active_at_timestamp(OptimismHardfork::Granite, 51));
    }

    #[test]
    fn parse_genesis_optimism_with_variable_base_fee_params() {
        use op_alloy_rpc_types::genesis::OptimismBaseFeeInfo;

        let geth_genesis = r#"
    {
      "config": {
        "chainId": 8453,
        "homesteadBlock": 0,
        "eip150Block": 0,
        "eip155Block": 0,
        "eip158Block": 0,
        "byzantiumBlock": 0,
        "constantinopleBlock": 0,
        "petersburgBlock": 0,
        "istanbulBlock": 0,
        "muirGlacierBlock": 0,
        "berlinBlock": 0,
        "londonBlock": 0,
        "arrowGlacierBlock": 0,
        "grayGlacierBlock": 0,
        "mergeNetsplitBlock": 0,
        "bedrockBlock": 0,
        "regolithTime": 15,
        "terminalTotalDifficulty": 0,
        "terminalTotalDifficultyPassed": true,
        "optimism": {
          "eip1559Elasticity": 6,
          "eip1559Denominator": 50
        }
      }
    }
    "#;
        let genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();
        let chainspec = ChainSpec::from(genesis.clone());

        let actual_chain_id = genesis.config.chain_id;
        assert_eq!(actual_chain_id, 8453);

        assert_eq!(
            chainspec.hardforks.get(EthereumHardfork::Istanbul),
            Some(ForkCondition::Block(0))
        );

        let actual_bedrock_block = genesis.config.extra_fields.get("bedrockBlock");
        assert_eq!(actual_bedrock_block, Some(serde_json::Value::from(0)).as_ref());
        let actual_canyon_timestamp = genesis.config.extra_fields.get("canyonTime");
        assert_eq!(actual_canyon_timestamp, None);

        assert!(genesis.config.terminal_total_difficulty_passed);

        let optimism_object = genesis.config.extra_fields.get("optimism").unwrap();
        let optimism_base_fee_info =
            serde_json::from_value::<OptimismBaseFeeInfo>(optimism_object.clone()).unwrap();

        assert_eq!(
            optimism_base_fee_info,
            OptimismBaseFeeInfo {
                eip1559_elasticity: Some(6),
                eip1559_denominator: Some(50),
                eip1559_denominator_canyon: None,
            }
        );
        assert_eq!(
            chainspec.base_fee_params,
            BaseFeeParamsKind::Constant(BaseFeeParams {
                max_change_denominator: 50,
                elasticity_multiplier: 6,
            })
        );

        assert!(chainspec.is_fork_active_at_block(OptimismHardfork::Bedrock, 0));

        assert!(chainspec.is_fork_active_at_timestamp(OptimismHardfork::Regolith, 20));
    }
}
