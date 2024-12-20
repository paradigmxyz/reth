//! OP-Reth chain specs.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod base;
mod base_sepolia;
pub mod constants;
mod dev;
mod op;
mod op_sepolia;

use alloc::{boxed::Box, vec, vec::Vec};
use alloy_chains::Chain;
use alloy_consensus::Header;
use alloy_genesis::Genesis;
use alloy_primitives::{B256, U256};
pub use base::BASE_MAINNET;
pub use base_sepolia::BASE_SEPOLIA;
use derive_more::{Constructor, Deref, Display, From, Into};
pub use dev::OP_DEV;
#[cfg(not(feature = "std"))]
pub(crate) use once_cell::sync::Lazy as LazyLock;
pub use op::OP_MAINNET;
use op_alloy_consensus::{decode_holocene_extra_data, EIP1559ParamError};
pub use op_sepolia::OP_SEPOLIA;
use reth_chainspec::{
    BaseFeeParams, BaseFeeParamsKind, ChainSpec, ChainSpecBuilder, DepositContract, EthChainSpec,
    EthereumHardforks, ForkFilter, ForkId, Hardforks, Head,
};
use reth_ethereum_forks::{ChainHardforks, EthereumHardfork, ForkCondition, Hardfork};
use reth_network_peers::NodeRecord;
use reth_optimism_forks::{OpHardfork, OpHardforks};
#[cfg(feature = "std")]
pub(crate) use std::sync::LazyLock;

/// Chain spec builder for a OP stack chain.
#[derive(Debug, Default, From)]
pub struct OpChainSpecBuilder {
    /// [`ChainSpecBuilder`]
    inner: ChainSpecBuilder,
}

impl OpChainSpecBuilder {
    /// Construct a new builder from the base mainnet chain spec.
    pub fn base_mainnet() -> Self {
        let mut inner = ChainSpecBuilder::default()
            .chain(BASE_MAINNET.chain)
            .genesis(BASE_MAINNET.genesis.clone());
        let forks = BASE_MAINNET.hardforks.clone();
        inner = inner.with_forks(forks);

        Self { inner }
    }

    /// Construct a new builder from the optimism mainnet chain spec.
    pub fn optimism_mainnet() -> Self {
        let mut inner =
            ChainSpecBuilder::default().chain(OP_MAINNET.chain).genesis(OP_MAINNET.genesis.clone());
        let forks = OP_MAINNET.hardforks.clone();
        inner = inner.with_forks(forks);

        Self { inner }
    }
}

impl OpChainSpecBuilder {
    /// Set the chain ID
    pub fn chain(mut self, chain: Chain) -> Self {
        self.inner = self.inner.chain(chain);
        self
    }

    /// Set the genesis block.
    pub fn genesis(mut self, genesis: Genesis) -> Self {
        self.inner = self.inner.genesis(genesis);
        self
    }

    /// Add the given fork with the given activation condition to the spec.
    pub fn with_fork<H: Hardfork>(mut self, fork: H, condition: ForkCondition) -> Self {
        self.inner = self.inner.with_fork(fork, condition);
        self
    }

    /// Add the given forks with the given activation condition to the spec.
    pub fn with_forks(mut self, forks: ChainHardforks) -> Self {
        self.inner = self.inner.with_forks(forks);
        self
    }

    /// Remove the given fork from the spec.
    pub fn without_fork(mut self, fork: reth_optimism_forks::OpHardfork) -> Self {
        self.inner = self.inner.without_fork(fork);
        self
    }

    /// Enable Bedrock at genesis
    pub fn bedrock_activated(mut self) -> Self {
        self.inner = self.inner.paris_activated();
        self.inner =
            self.inner.with_fork(reth_optimism_forks::OpHardfork::Bedrock, ForkCondition::Block(0));
        self
    }

    /// Enable Regolith at genesis
    pub fn regolith_activated(mut self) -> Self {
        self = self.bedrock_activated();
        self.inner = self
            .inner
            .with_fork(reth_optimism_forks::OpHardfork::Regolith, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Canyon at genesis
    pub fn canyon_activated(mut self) -> Self {
        self = self.regolith_activated();
        // Canyon also activates changes from L1's Shanghai hardfork
        self.inner = self.inner.with_fork(EthereumHardfork::Shanghai, ForkCondition::Timestamp(0));
        self.inner = self
            .inner
            .with_fork(reth_optimism_forks::OpHardfork::Canyon, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Ecotone at genesis
    pub fn ecotone_activated(mut self) -> Self {
        self = self.canyon_activated();
        self.inner = self.inner.with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(0));
        self.inner = self
            .inner
            .with_fork(reth_optimism_forks::OpHardfork::Ecotone, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Fjord at genesis
    pub fn fjord_activated(mut self) -> Self {
        self = self.ecotone_activated();
        self.inner = self
            .inner
            .with_fork(reth_optimism_forks::OpHardfork::Fjord, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Granite at genesis
    pub fn granite_activated(mut self) -> Self {
        self = self.fjord_activated();
        self.inner = self
            .inner
            .with_fork(reth_optimism_forks::OpHardfork::Granite, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Holocene at genesis
    pub fn holocene_activated(mut self) -> Self {
        self = self.granite_activated();
        self.inner = self
            .inner
            .with_fork(reth_optimism_forks::OpHardfork::Holocene, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Isthmus at genesis
    pub fn isthmus_activated(mut self) -> Self {
        self = self.holocene_activated();
        self.inner = self.inner.with_fork(OpHardfork::Isthmus, ForkCondition::Timestamp(0));
        self
    }

    /// Build the resulting [`OpChainSpec`].
    ///
    /// # Panics
    ///
    /// This function panics if the chain ID and genesis is not set ([`Self::chain`] and
    /// [`Self::genesis`])
    pub fn build(self) -> OpChainSpec {
        OpChainSpec { inner: self.inner.build() }
    }
}

/// OP stack chain spec type.
#[derive(Debug, Clone, Deref, Into, Constructor, PartialEq, Eq)]
pub struct OpChainSpec {
    /// [`ChainSpec`].
    pub inner: ChainSpec,
}

impl OpChainSpec {
    /// Extracts the Holocene 1599 parameters from the encoded extra data from the parent header.
    ///
    /// Caution: Caller must ensure that holocene is active in the parent header.
    ///
    /// See also [Base fee computation](https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/holocene/exec-engine.md#base-fee-computation)
    pub fn decode_holocene_base_fee(
        &self,
        parent: &Header,
        timestamp: u64,
    ) -> Result<u64, EIP1559ParamError> {
        let (denominator, elasticity) = decode_holocene_extra_data(&parent.extra_data)?;
        let base_fee = if elasticity == 0 && denominator == 0 {
            parent
                .next_block_base_fee(self.base_fee_params_at_timestamp(timestamp))
                .unwrap_or_default()
        } else {
            let base_fee_params = BaseFeeParams::new(denominator as u128, elasticity as u128);
            parent.next_block_base_fee(base_fee_params).unwrap_or_default()
        };
        Ok(base_fee)
    }

    /// Read from parent to determine the base fee for the next block
    ///
    /// See also [Base fee computation](https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/holocene/exec-engine.md#base-fee-computation)
    pub fn next_block_base_fee(
        &self,
        parent: &Header,
        timestamp: u64,
    ) -> Result<U256, EIP1559ParamError> {
        // > if Holocene is active in parent_header.timestamp, then the parameters from
        // > parent_header.extraData are used.
        let is_holocene_activated =
            self.inner.is_fork_active_at_timestamp(OpHardfork::Holocene, parent.timestamp);

        // If we are in the Holocene, we need to use the base fee params
        // from the parent block's extra data.
        // Else, use the base fee params (default values) from chainspec
        if is_holocene_activated {
            Ok(U256::from(self.decode_holocene_base_fee(parent, timestamp)?))
        } else {
            Ok(U256::from(
                parent
                    .next_block_base_fee(self.base_fee_params_at_timestamp(timestamp))
                    .unwrap_or_default(),
            ))
        }
    }
}

impl EthChainSpec for OpChainSpec {
    type Header = Header;

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

    fn display_hardforks(&self) -> Box<dyn Display> {
        Box::new(ChainSpec::display_hardforks(self))
    }

    fn genesis_header(&self) -> &Self::Header {
        self.inner.genesis_header()
    }

    fn genesis(&self) -> &Genesis {
        self.inner.genesis()
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

impl OpHardforks for OpChainSpec {}

impl From<Genesis> for OpChainSpec {
    fn from(genesis: Genesis) -> Self {
        use reth_optimism_forks::OpHardfork;
        let optimism_genesis_info = OpGenesisInfo::extract_from(&genesis);
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
            (OpHardfork::Bedrock.boxed(), genesis_info.bedrock_block),
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
            (OpHardfork::Regolith.boxed(), genesis_info.regolith_time),
            (OpHardfork::Canyon.boxed(), genesis_info.canyon_time),
            (OpHardfork::Ecotone.boxed(), genesis_info.ecotone_time),
            (OpHardfork::Fjord.boxed(), genesis_info.fjord_time),
            (OpHardfork::Granite.boxed(), genesis_info.granite_time),
            (OpHardfork::Holocene.boxed(), genesis_info.holocene_time),
            (OpHardfork::Isthmus.boxed(), genesis_info.isthmus_time),
        ];

        let mut time_hardforks = time_hardfork_opts
            .into_iter()
            .filter_map(|(hardfork, opt)| {
                opt.map(|time| (hardfork, ForkCondition::Timestamp(time)))
            })
            .collect::<Vec<_>>();

        block_hardforks.append(&mut time_hardforks);

        // Ordered Hardforks
        let mainnet_hardforks = OpHardfork::op_mainnet();
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
                hardforks: ChainHardforks::new(ordered_hardforks),
                paris_block_and_final_difficulty,
                base_fee_params: optimism_genesis_info.base_fee_params,
                ..Default::default()
            },
        }
    }
}

#[derive(Default, Debug)]
struct OpGenesisInfo {
    optimism_chain_info: op_alloy_rpc_types::OpChainInfo,
    base_fee_params: BaseFeeParamsKind,
}

impl OpGenesisInfo {
    fn extract_from(genesis: &Genesis) -> Self {
        let mut info = Self {
            optimism_chain_info: op_alloy_rpc_types::OpChainInfo::extract_from(
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
                                reth_optimism_forks::OpHardfork::Canyon.boxed(),
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
    use std::sync::Arc;

    use alloy_genesis::{ChainConfig, Genesis};
    use alloy_primitives::{b256, Bytes};
    use reth_chainspec::{test_fork_ids, BaseFeeParams, BaseFeeParamsKind};
    use reth_ethereum_forks::{EthereumHardfork, ForkCondition, ForkHash, ForkId, Head};
    use reth_optimism_forks::{OpHardfork, OpHardforks};

    use crate::*;

    #[test]
    fn base_mainnet_forkids() {
        let base_mainnet = OpChainSpecBuilder::base_mainnet().build();
        let _ = base_mainnet.genesis_hash.set(BASE_MAINNET.genesis_hash.get().copied().unwrap());
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
                    ForkId { hash: ForkHash([0xbc, 0x38, 0xf9, 0xca]), next: 1736445601 },
                ),
                (
                    Head { number: 0, timestamp: 1736445601, ..Default::default() },
                    ForkId { hash: ForkHash([0x3a, 0x2a, 0xf1, 0x83]), next: 0 },
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
                    ForkId { hash: ForkHash([0x75, 0xde, 0xa4, 0x1e]), next: 1732633200 },
                ),
                (
                    Head { number: 0, timestamp: 1732633200, ..Default::default() },
                    ForkId { hash: ForkHash([0x4a, 0x1c, 0x79, 0x2e]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn op_mainnet_forkids() {
        let op_mainnet = OpChainSpecBuilder::optimism_mainnet().build();
        // for OP mainnet we have to do this because the genesis header can't be properly computed
        // from the genesis.json file
        let _ = op_mainnet.genesis_hash.set(OP_MAINNET.genesis_hash());
        test_fork_ids(
            &op_mainnet,
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
                    ForkId { hash: ForkHash([0x5e, 0xdf, 0xa3, 0xb6]), next: 1732633200 },
                ),
                (
                    Head { number: 0, timestamp: 1732633200, ..Default::default() },
                    ForkId { hash: ForkHash([0x8b, 0x5e, 0x76, 0x29]), next: 0 },
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
            ForkId { hash: ForkHash([0x3a, 0x2a, 0xf1, 0x83]), next: 0 },
            BASE_MAINNET.latest_fork_id()
        )
    }

    #[test]
    fn latest_base_mainnet_fork_id_with_builder() {
        let base_mainnet = OpChainSpecBuilder::base_mainnet().build();
        assert_eq!(
            ForkId { hash: ForkHash([0x3a, 0x2a, 0xf1, 0x83]), next: 0 },
            base_mainnet.latest_fork_id()
        )
    }

    #[test]
    fn is_bedrock_active() {
        let op_mainnet = OpChainSpecBuilder::optimism_mainnet().build();
        assert!(!op_mainnet.is_bedrock_active_at_block(1))
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
        "holoceneTime": 52,
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
        let actual_holocene_timestamp = genesis.config.extra_fields.get("holoceneTime");
        assert_eq!(actual_holocene_timestamp, Some(serde_json::Value::from(52)).as_ref());

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

        assert!(!chain_spec.is_fork_active_at_block(OpHardfork::Bedrock, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OpHardfork::Regolith, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OpHardfork::Canyon, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OpHardfork::Ecotone, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OpHardfork::Fjord, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OpHardfork::Granite, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OpHardfork::Holocene, 0));

        assert!(chain_spec.is_fork_active_at_block(OpHardfork::Bedrock, 10));
        assert!(chain_spec.is_fork_active_at_timestamp(OpHardfork::Regolith, 20));
        assert!(chain_spec.is_fork_active_at_timestamp(OpHardfork::Canyon, 30));
        assert!(chain_spec.is_fork_active_at_timestamp(OpHardfork::Ecotone, 40));
        assert!(chain_spec.is_fork_active_at_timestamp(OpHardfork::Fjord, 50));
        assert!(chain_spec.is_fork_active_at_timestamp(OpHardfork::Granite, 51));
        assert!(chain_spec.is_fork_active_at_timestamp(OpHardfork::Holocene, 52));
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
        "holoceneTime": 52,
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
        let actual_holocene_timestamp = genesis.config.extra_fields.get("holoceneTime");
        assert_eq!(actual_holocene_timestamp, Some(serde_json::Value::from(52)).as_ref());

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
                    (OpHardfork::Canyon.boxed(), BaseFeeParams::new(80, 60)),
                ]
                .into()
            )
        );

        assert!(!chain_spec.is_fork_active_at_block(OpHardfork::Bedrock, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OpHardfork::Regolith, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OpHardfork::Canyon, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OpHardfork::Ecotone, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OpHardfork::Fjord, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OpHardfork::Granite, 0));
        assert!(!chain_spec.is_fork_active_at_timestamp(OpHardfork::Holocene, 0));

        assert!(chain_spec.is_fork_active_at_block(OpHardfork::Bedrock, 10));
        assert!(chain_spec.is_fork_active_at_timestamp(OpHardfork::Regolith, 20));
        assert!(chain_spec.is_fork_active_at_timestamp(OpHardfork::Canyon, 30));
        assert!(chain_spec.is_fork_active_at_timestamp(OpHardfork::Ecotone, 40));
        assert!(chain_spec.is_fork_active_at_timestamp(OpHardfork::Fjord, 50));
        assert!(chain_spec.is_fork_active_at_timestamp(OpHardfork::Granite, 51));
        assert!(chain_spec.is_fork_active_at_timestamp(OpHardfork::Holocene, 52));
    }

    #[test]
    fn parse_genesis_optimism_with_variable_base_fee_params() {
        use op_alloy_rpc_types::OpBaseFeeInfo;

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

        assert!(chainspec.is_fork_active_at_block(OpHardfork::Bedrock, 0));

        assert!(chainspec.is_fork_active_at_timestamp(OpHardfork::Regolith, 20));
    }

    #[test]
    fn test_fork_order_optimism_mainnet() {
        use reth_optimism_forks::OpHardfork;

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
                    (String::from("holoceneTime"), 0.into()),
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
            OpHardfork::Bedrock.boxed(),
            OpHardfork::Regolith.boxed(),
            EthereumHardfork::Shanghai.boxed(),
            OpHardfork::Canyon.boxed(),
            EthereumHardfork::Cancun.boxed(),
            OpHardfork::Ecotone.boxed(),
            OpHardfork::Fjord.boxed(),
            OpHardfork::Granite.boxed(),
            OpHardfork::Holocene.boxed(),
            // OpHardfork::Isthmus.boxed(),
        ];

        assert!(expected_hardforks
            .iter()
            .zip(hardforks.iter())
            .all(|(expected, actual)| &**expected == *actual));
        assert_eq!(expected_hardforks.len(), hardforks.len());
    }

    #[test]
    fn test_get_base_fee_pre_holocene() {
        let op_chain_spec = &BASE_SEPOLIA;
        let parent = Header {
            base_fee_per_gas: Some(1),
            gas_used: 15763614,
            gas_limit: 144000000,
            ..Default::default()
        };
        let base_fee = op_chain_spec.next_block_base_fee(&parent, 0);
        assert_eq!(
            base_fee.unwrap(),
            U256::from(
                parent
                    .next_block_base_fee(op_chain_spec.base_fee_params_at_timestamp(0))
                    .unwrap_or_default()
            )
        );
    }

    fn holocene_chainspec() -> Arc<OpChainSpec> {
        let mut hardforks = OpHardfork::base_sepolia();
        hardforks.insert(OpHardfork::Holocene.boxed(), ForkCondition::Timestamp(1800000000));
        Arc::new(OpChainSpec {
            inner: ChainSpec {
                chain: BASE_SEPOLIA.inner.chain,
                genesis: BASE_SEPOLIA.inner.genesis.clone(),
                genesis_hash: BASE_SEPOLIA.inner.genesis_hash.clone(),
                paris_block_and_final_difficulty: Some((0, U256::from(0))),
                hardforks,
                base_fee_params: BASE_SEPOLIA.inner.base_fee_params.clone(),
                prune_delete_limit: 10000,
                ..Default::default()
            },
        })
    }

    #[test]
    fn test_get_base_fee_holocene_extra_data_not_set() {
        let op_chain_spec = holocene_chainspec();
        let parent = Header {
            base_fee_per_gas: Some(1),
            gas_used: 15763614,
            gas_limit: 144000000,
            timestamp: 1800000003,
            extra_data: Bytes::from_static(&[0, 0, 0, 0, 0, 0, 0, 0, 0]),
            ..Default::default()
        };
        let base_fee = op_chain_spec.next_block_base_fee(&parent, 1800000005);
        assert_eq!(
            base_fee.unwrap(),
            U256::from(
                parent
                    .next_block_base_fee(op_chain_spec.base_fee_params_at_timestamp(0))
                    .unwrap_or_default()
            )
        );
    }

    #[test]
    fn test_get_base_fee_holocene_extra_data_set() {
        let op_chain_spec = holocene_chainspec();
        let parent = Header {
            base_fee_per_gas: Some(1),
            gas_used: 15763614,
            gas_limit: 144000000,
            extra_data: Bytes::from_static(&[0, 0, 0, 0, 8, 0, 0, 0, 8]),
            timestamp: 1800000003,
            ..Default::default()
        };

        let base_fee = op_chain_spec.next_block_base_fee(&parent, 1800000005);
        assert_eq!(
            base_fee.unwrap(),
            U256::from(
                parent
                    .next_block_base_fee(BaseFeeParams::new(0x00000008, 0x00000008))
                    .unwrap_or_default()
            )
        );
    }
}
