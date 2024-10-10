//! OP-Reth chain specs.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

extern crate alloc;

mod base;
mod base_sepolia;
pub mod constants;
mod dev;
mod op;
mod op_sepolia;

use std::fmt::Display;

use alloy_genesis::Genesis;
use alloy_primitives::{Parity, Signature, B256, U256};
pub use base::BASE_MAINNET;
pub use base_sepolia::BASE_SEPOLIA;
pub use dev::OP_DEV;
pub use op::OP_MAINNET;
pub use op_sepolia::OP_SEPOLIA;

use derive_more::{Constructor, Deref, Into};
use once_cell::sync::OnceCell;
use reth_chainspec::{
    BaseFeeParams, BaseFeeParamsKind, ChainSpec, DepositContract, EthChainSpec, EthereumHardforks,
    ForkFilter, ForkId, Hardforks, Head,
};
use reth_ethereum_forks::{ChainHardforks, EthereumHardfork, ForkCondition};
use reth_network_peers::NodeRecord;
use reth_primitives_traits::Header;

/// OP stack chain spec type.
#[derive(Debug, Clone, Deref, Into, Constructor, PartialEq, Eq)]
pub struct OpChainSpec {
    /// [`ChainSpec`].
    pub inner: ChainSpec,
}

/// Returns the signature for the optimism deposit transactions, which don't include a
/// signature.
pub fn optimism_deposit_tx_signature() -> Signature {
    Signature::new(U256::ZERO, U256::ZERO, Parity::Parity(false))
}

impl EthChainSpec for OpChainSpec {
    fn chain(&self) -> alloy_chains::Chain {
        self.inner.chain()
    }

    fn base_fee_params_at_block(&self, block_number: u64) -> BaseFeeParams {
        self.inner.base_fee_params_at_block(block_number)
    }

    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> BaseFeeParams {
        self.inner.base_fee_params_at_timestamp(timestamp)
    }

    fn deposit_contract(&self) -> Option<&DepositContract> {
        self.inner.deposit_contract()
    }

    fn genesis_hash(&self) -> B256 {
        self.inner.genesis_hash()
    }

    fn prune_delete_limit(&self) -> usize {
        self.inner.prune_delete_limit()
    }

    fn display_hardforks(&self) -> impl Display {
        self.inner.display_hardforks()
    }

    fn genesis_header(&self) -> &Header {
        self.inner.genesis_header()
    }

    fn genesis(&self) -> &Genesis {
        self.inner.genesis()
    }

    fn max_gas_limit(&self) -> u64 {
        self.inner.max_gas_limit()
    }

    fn bootnodes(&self) -> Option<Vec<NodeRecord>> {
        self.inner.bootnodes()
    }

    fn is_optimism(&self) -> bool {
        true
    }
}

impl Hardforks for OpChainSpec {
    fn fork<H: reth_chainspec::Hardfork>(&self, fork: H) -> reth_chainspec::ForkCondition {
        self.inner.fork(fork)
    }

    fn forks_iter(
        &self,
    ) -> impl Iterator<Item = (&dyn reth_chainspec::Hardfork, reth_chainspec::ForkCondition)> {
        self.inner.forks_iter()
    }

    fn fork_id(&self, head: &Head) -> ForkId {
        self.inner.fork_id(head)
    }

    fn latest_fork_id(&self) -> ForkId {
        self.inner.latest_fork_id()
    }

    fn fork_filter(&self, head: Head) -> ForkFilter {
        self.inner.fork_filter(head)
    }
}

impl EthereumHardforks for OpChainSpec {
    fn get_final_paris_total_difficulty(&self) -> Option<U256> {
        self.inner.get_final_paris_total_difficulty()
    }

    fn final_paris_total_difficulty(&self, block_number: u64) -> Option<U256> {
        self.inner.final_paris_total_difficulty(block_number)
    }
}

impl From<Genesis> for OpChainSpec {
    fn from(genesis: Genesis) -> Self {
        use reth_optimism_forks::OptimismHardfork;
        let optimism_genesis_info = OptimismGenesisInfo::extract_from(&genesis);
        let genesis_info =
            optimism_genesis_info.optimism_chain_info.genesis_info.unwrap_or_default();

        // Block-based hardforks
        let hardfork_opts = [
            (EthereumHardfork::Homestead.boxed(), genesis.config.homestead_block),
            (EthereumHardfork::Tangerine.boxed(), genesis.config.eip150_block),
            (EthereumHardfork::SpuriousDragon.boxed(), genesis.config.eip155_block),
            (EthereumHardfork::Byzantium.boxed(), genesis.config.byzantium_block),
            (EthereumHardfork::Constantinople.boxed(), genesis.config.constantinople_block),
            (EthereumHardfork::Petersburg.boxed(), genesis.config.petersburg_block),
            (EthereumHardfork::Istanbul.boxed(), genesis.config.istanbul_block),
            (EthereumHardfork::MuirGlacier.boxed(), genesis.config.muir_glacier_block),
            (EthereumHardfork::Berlin.boxed(), genesis.config.berlin_block),
            (EthereumHardfork::London.boxed(), genesis.config.london_block),
            (EthereumHardfork::ArrowGlacier.boxed(), genesis.config.arrow_glacier_block),
            (EthereumHardfork::GrayGlacier.boxed(), genesis.config.gray_glacier_block),
            (OptimismHardfork::Bedrock.boxed(), genesis_info.bedrock_block),
        ];
        let mut block_hardforks = hardfork_opts
            .into_iter()
            .filter_map(|(hardfork, opt)| opt.map(|block| (hardfork, ForkCondition::Block(block))))
            .collect::<Vec<_>>();

        // Paris
        let paris_block_and_final_difficulty =
            if let Some(ttd) = genesis.config.terminal_total_difficulty {
                block_hardforks.push((
                    EthereumHardfork::Paris.boxed(),
                    ForkCondition::TTD {
                        total_difficulty: ttd,
                        fork_block: genesis.config.merge_netsplit_block,
                    },
                ));

                genesis.config.merge_netsplit_block.map(|block| (block, ttd))
            } else {
                None
            };

        // Time-based hardforks
        let time_hardfork_opts = [
            (EthereumHardfork::Shanghai.boxed(), genesis.config.shanghai_time),
            (EthereumHardfork::Cancun.boxed(), genesis.config.cancun_time),
            (EthereumHardfork::Prague.boxed(), genesis.config.prague_time),
            (OptimismHardfork::Regolith.boxed(), genesis_info.regolith_time),
            (OptimismHardfork::Canyon.boxed(), genesis_info.canyon_time),
            (OptimismHardfork::Ecotone.boxed(), genesis_info.ecotone_time),
            (OptimismHardfork::Fjord.boxed(), genesis_info.fjord_time),
            (OptimismHardfork::Granite.boxed(), genesis_info.granite_time),
        ];

        let mut time_hardforks = time_hardfork_opts
            .into_iter()
            .filter_map(|(hardfork, opt)| {
                opt.map(|time| (hardfork, ForkCondition::Timestamp(time)))
            })
            .collect::<Vec<_>>();

        block_hardforks.append(&mut time_hardforks);

        // Ordered Hardforks
        let mainnet_hardforks = OptimismHardfork::op_mainnet();
        let mainnet_order = mainnet_hardforks.forks_iter();

        let mut ordered_hardforks = Vec::with_capacity(block_hardforks.len());
        for (hardfork, _) in mainnet_order {
            if let Some(pos) = block_hardforks.iter().position(|(e, _)| **e == *hardfork) {
                ordered_hardforks.push(block_hardforks.remove(pos));
            }
        }

        // append the remaining unknown hardforks to ensure we don't filter any out
        ordered_hardforks.append(&mut block_hardforks);

        Self {
            inner: ChainSpec {
                chain: genesis.config.chain_id.into(),
                genesis,
                genesis_hash: OnceCell::new(),
                hardforks: ChainHardforks::new(ordered_hardforks),
                paris_block_and_final_difficulty,
                base_fee_params: optimism_genesis_info.base_fee_params,
                ..Default::default()
            },
        }
    }
}

#[derive(Default, Debug)]
struct OptimismGenesisInfo {
    optimism_chain_info: op_alloy_rpc_types::genesis::OpChainInfo,
    base_fee_params: BaseFeeParamsKind,
}

impl OptimismGenesisInfo {
    fn extract_from(genesis: &Genesis) -> Self {
        let mut info = Self {
            optimism_chain_info: op_alloy_rpc_types::genesis::OpChainInfo::extract_from(
                &genesis.config.extra_fields,
            )
            .unwrap_or_default(),
            ..Default::default()
        };
        if let Some(optimism_base_fee_info) = &info.optimism_chain_info.base_fee_info {
            if let (Some(elasticity), Some(denominator)) = (
                optimism_base_fee_info.eip1559_elasticity,
                optimism_base_fee_info.eip1559_denominator,
            ) {
                let base_fee_params = if let Some(canyon_denominator) =
                    optimism_base_fee_info.eip1559_denominator_canyon
                {
                    BaseFeeParamsKind::Variable(
                        vec![
                            (
                                EthereumHardfork::London.boxed(),
                                BaseFeeParams::new(denominator as u128, elasticity as u128),
                            ),
                            (
                                reth_optimism_forks::OptimismHardfork::Canyon.boxed(),
                                BaseFeeParams::new(canyon_denominator as u128, elasticity as u128),
                            ),
                        ]
                        .into(),
                    )
                } else {
                    BaseFeeParams::new(denominator as u128, elasticity as u128).into()
                };

                info.base_fee_params = base_fee_params;
            }
        }

        info
    }
}

#[cfg(test)]
mod tests {
    use alloy_genesis::{ChainConfig, Genesis};
    use alloy_primitives::b256;
    use reth_chainspec::{test_fork_ids, BaseFeeParams, BaseFeeParamsKind};
    use reth_ethereum_forks::{EthereumHardfork, ForkCondition, ForkHash, ForkId, Head};
    use reth_optimism_forks::{OptimismHardfork, OptimismHardforks};

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

        let chain_spec: OpChainSpec = genesis.into();

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

        let chain_spec: OpChainSpec = genesis.into();

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
        use op_alloy_rpc_types::genesis::OpBaseFeeInfo;

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
        let chainspec = OpChainSpec::from(genesis.clone());

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
            serde_json::from_value::<OpBaseFeeInfo>(optimism_object.clone()).unwrap();

        assert_eq!(
            optimism_base_fee_info,
            OpBaseFeeInfo {
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

    #[test]
    fn test_fork_order_optimism_mainnet() {
        use reth_optimism_forks::OptimismHardfork;

        let genesis = Genesis {
            config: ChainConfig {
                chain_id: 0,
                homestead_block: Some(0),
                dao_fork_block: Some(0),
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
                shanghai_time: Some(0),
                cancun_time: Some(0),
                terminal_total_difficulty: Some(U256::ZERO),
                extra_fields: [
                    (String::from("bedrockBlock"), 0.into()),
                    (String::from("regolithTime"), 0.into()),
                    (String::from("canyonTime"), 0.into()),
                    (String::from("ecotoneTime"), 0.into()),
                    (String::from("fjordTime"), 0.into()),
                    (String::from("graniteTime"), 0.into()),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
            ..Default::default()
        };

        let chain_spec: OpChainSpec = genesis.into();

        let hardforks: Vec<_> = chain_spec.hardforks.forks_iter().map(|(h, _)| h).collect();
        let expected_hardforks = vec![
            EthereumHardfork::Homestead.boxed(),
            EthereumHardfork::Tangerine.boxed(),
            EthereumHardfork::SpuriousDragon.boxed(),
            EthereumHardfork::Byzantium.boxed(),
            EthereumHardfork::Constantinople.boxed(),
            EthereumHardfork::Petersburg.boxed(),
            EthereumHardfork::Istanbul.boxed(),
            EthereumHardfork::MuirGlacier.boxed(),
            EthereumHardfork::Berlin.boxed(),
            EthereumHardfork::London.boxed(),
            EthereumHardfork::ArrowGlacier.boxed(),
            EthereumHardfork::GrayGlacier.boxed(),
            EthereumHardfork::Paris.boxed(),
            OptimismHardfork::Bedrock.boxed(),
            OptimismHardfork::Regolith.boxed(),
            EthereumHardfork::Shanghai.boxed(),
            OptimismHardfork::Canyon.boxed(),
            EthereumHardfork::Cancun.boxed(),
            OptimismHardfork::Ecotone.boxed(),
            OptimismHardfork::Fjord.boxed(),
            OptimismHardfork::Granite.boxed(),
        ];

        assert!(expected_hardforks
            .iter()
            .zip(hardforks.iter())
            .all(|(expected, actual)| &**expected == *actual));
        assert_eq!(expected_hardforks.len(), hardforks.len());
    }
}
