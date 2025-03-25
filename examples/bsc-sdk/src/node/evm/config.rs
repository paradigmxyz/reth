use super::factory::BscEvmFactory;
use crate::{
    chainspec::BscChainSpec, evm::spec::BscSpecId, hardforks::bsc::BscHardfork, node::evm::BscEvm,
};
use alloy_consensus::{BlockHeader, Header};
use alloy_primitives::{BlockNumber, Bytes, U256};
use reth_chainspec::EthChainSpec;
use reth_ethereum_forks::EthereumHardfork;
use reth_evm::{
    block::{BlockExecutorFactory, BlockExecutorFor},
    eth::{EthBlockExecutionCtx, EthBlockExecutor, EthBlockExecutorFactory},
    ConfigureEvm, EvmEnv, ExecutionCtxFor, InspectorFor, NextBlockEnvAttributes,
};
use reth_evm_ethereum::{EthBlockAssembler, RethReceiptBuilder};
use reth_primitives::{
    BlockTy, EthPrimitives, HeaderTy, Receipt, SealedBlock, SealedHeader, TransactionSigned,
};
use reth_revm::{Database, State};
use revm::{
    context::{BlockEnv, CfgEnv},
    context_interface::block::BlobExcessGasAndPrice,
    primitives::hardfork::SpecId,
};
use std::{borrow::Cow, convert::Infallible, sync::Arc};

/// Ethereum-related EVM configuration.
#[derive(Debug, Clone)]
pub struct BscEvmConfig {
    /// Inner [`EthBlockExecutorFactory`].
    pub executor_factory:
        EthBlockExecutorFactory<RethReceiptBuilder, Arc<BscChainSpec>, BscEvmFactory>,
    /// Ethereum block assembler.
    pub block_assembler: EthBlockAssembler<BscChainSpec>,
}

impl BscEvmConfig {
    /// Creates a new Ethereum EVM configuration with the given chain spec.
    pub fn new(chain_spec: Arc<BscChainSpec>) -> Self {
        Self::bsc(chain_spec)
    }

    /// Creates a new Ethereum EVM configuration.
    pub fn bsc(chain_spec: Arc<BscChainSpec>) -> Self {
        Self::new_with_evm_factory(chain_spec, BscEvmFactory::default())
    }
}

impl BscEvmConfig {
    /// Creates a new Ethereum EVM configuration with the given chain spec and EVM factory.
    pub fn new_with_evm_factory(chain_spec: Arc<BscChainSpec>, evm_factory: BscEvmFactory) -> Self {
        Self {
            block_assembler: EthBlockAssembler::new(chain_spec.clone()),
            executor_factory: EthBlockExecutorFactory::new(
                RethReceiptBuilder::default(),
                chain_spec,
                evm_factory,
            ),
        }
    }

    /// Returns the chain spec associated with this configuration.
    pub const fn chain_spec(&self) -> &Arc<BscChainSpec> {
        self.executor_factory.spec()
    }

    /// Sets the extra data for the block assembler.
    pub fn with_extra_data(mut self, extra_data: Bytes) -> Self {
        self.block_assembler.extra_data = extra_data;
        self
    }
}

impl BlockExecutorFactory for BscEvmConfig {
    type EvmFactory = BscEvmFactory;
    type ExecutionCtx<'a> = EthBlockExecutionCtx<'a>;
    type Transaction = TransactionSigned;
    type Receipt = Receipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.executor_factory.evm_factory()
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: BscEvm<&'a mut State<DB>, I>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: Database + 'a,
        I: InspectorFor<Self, &'a mut State<DB>> + 'a,
        DB::Error: Send + Sync + 'static,
    {
        EthBlockExecutor::new(evm, ctx, self.chain_spec(), self.executor_factory.receipt_builder())
    }
}

const EIP1559_INITIAL_BASE_FEE: u64 = 0;

impl ConfigureEvm for BscEvmConfig
where
    Self: Send + Sync + Unpin + Clone + 'static,
{
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type BlockExecutorFactory =
        EthBlockExecutorFactory<RethReceiptBuilder, Arc<BscChainSpec>, BscEvmFactory>;
    type BlockAssembler = EthBlockAssembler<BscChainSpec>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn evm_env(&self, header: &Header) -> EvmEnv<BscSpecId> {
        let spec = revm_spec(self.chain_spec().clone(), header.number());

        let cfg_env = CfgEnv::new().with_chain_id(self.chain_spec().chain().id()).with_spec(spec);

        let block_env = BlockEnv {
            number: header.number(),
            beneficiary: header.beneficiary(),
            timestamp: header.timestamp(),
            difficulty: U256::ZERO,
            prevrandao: header.mix_hash(),
            gas_limit: header.gas_limit(),
            basefee: header.base_fee_per_gas().unwrap_or_default(),
            // EIP-4844 excess blob gas of this block, introduced in Cancun
            blob_excess_gas_and_price: header.excess_blob_gas().map(|excess_blob_gas| {
                BlobExcessGasAndPrice::new(excess_blob_gas, spec.into_eth_spec() >= SpecId::PRAGUE)
            }),
        };

        EvmEnv { cfg_env, block_env }
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnv<BscSpecId>, Self::Error> {
        // ensure we're not missing any timestamp based hardforks
        let spec_id =
            revm_spec_by_timestamp_after_shanghai(self.chain_spec().clone(), attributes.timestamp);

        // configure evm env based on parent block
        let cfg_env =
            CfgEnv::new().with_chain_id(self.chain_spec().chain().id()).with_spec(spec_id);

        // if the parent block did not have excess blob gas (i.e. it was pre-cancun), but it is
        // cancun now, we need to set the excess blob gas to the default value(0)
        let blob_excess_gas_and_price = parent
            .maybe_next_block_excess_blob_gas(
                self.chain_spec().blob_params_at_timestamp(attributes.timestamp),
            )
            .or_else(|| (spec_id.into_eth_spec().is_enabled_in(SpecId::CANCUN)).then_some(0))
            .map(|gas| BlobExcessGasAndPrice::new(gas, false));

        let mut basefee = parent.next_block_base_fee(
            self.chain_spec().base_fee_params_at_timestamp(attributes.timestamp),
        );

        let mut gas_limit = U256::from(parent.gas_limit);

        // If we are on the London fork boundary, we need to multiply the parent's gas limit by the
        // elasticity multiplier to get the new gas limit.
        if self
            .chain_spec()
            .inner
            .fork(EthereumHardfork::London)
            .transitions_at_block(parent.number + 1)
        {
            let elasticity_multiplier = self
                .chain_spec()
                .base_fee_params_at_timestamp(attributes.timestamp)
                .elasticity_multiplier;

            // multiply the gas limit by the elasticity multiplier
            gas_limit *= U256::from(elasticity_multiplier);

            // set the base fee to the initial base fee from the EIP-1559 spec
            basefee = Some(EIP1559_INITIAL_BASE_FEE)
        }

        let block_env = BlockEnv {
            number: parent.number() + 1,
            beneficiary: attributes.suggested_fee_recipient,
            timestamp: attributes.timestamp,
            difficulty: U256::ZERO,
            prevrandao: Some(attributes.prev_randao),
            gas_limit: attributes.gas_limit,
            // calculate basefee based on parent block's gas usage
            basefee: basefee.unwrap_or_default(),
            // calculate excess gas based on parent block's blob gas usage
            blob_excess_gas_and_price,
        };

        Ok(EvmEnv { cfg_env, block_env })
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<BlockTy<Self::Primitives>>,
    ) -> ExecutionCtxFor<'a, Self> {
        EthBlockExecutionCtx {
            parent_hash: block.header().parent_hash,
            parent_beacon_block_root: block.header().parent_beacon_block_root,
            ommers: &block.body().ommers,
            withdrawals: block.body().withdrawals.as_ref().map(Cow::Borrowed),
        }
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
    ) -> ExecutionCtxFor<'_, Self> {
        EthBlockExecutionCtx {
            parent_hash: parent.hash(),
            parent_beacon_block_root: attributes.parent_beacon_block_root,
            ommers: &[],
            withdrawals: attributes.withdrawals.map(Cow::Owned),
        }
    }
}

/// Returns the revm [`BscSpecId`] at the given timestamp.
///
/// # Note
///
/// This is only intended to be used after the Shangai, when hardforks are activated by
/// timestamp.
pub fn revm_spec_by_timestamp_after_shanghai(
    chain_spec: Arc<BscChainSpec>,
    timestamp: u64,
) -> BscSpecId {
    let chain_spec = chain_spec.inner.clone();
    if chain_spec.fork(BscHardfork::Bohr).active_at_timestamp(timestamp) {
        BscSpecId::BOHR
    } else if chain_spec.fork(BscHardfork::HaberFix).active_at_timestamp(timestamp) {
        BscSpecId::HABER_FIX
    } else if chain_spec.fork(BscHardfork::Haber).active_at_timestamp(timestamp) {
        BscSpecId::HABER
    } else if chain_spec.fork(BscHardfork::FeynmanFix).active_at_timestamp(timestamp) {
        BscSpecId::FEYNMAN_FIX
    } else if chain_spec.fork(BscHardfork::Feynman).active_at_timestamp(timestamp) {
        BscSpecId::FEYNMAN
    } else if chain_spec.fork(BscHardfork::Kepler).active_at_timestamp(timestamp) {
        BscSpecId::KEPLER
    } else {
        BscSpecId::SHANGHAI
    }
}

/// Returns the revm [`BscSpecId`] at the given block number.
///
/// # Note
///
/// This is only intended to be used before the Shangai, when hardforks are activated by
/// block number.
pub fn revm_spec(chain_spec: Arc<BscChainSpec>, block: BlockNumber) -> BscSpecId {
    let chain_spec = chain_spec.inner.clone();
    if chain_spec.fork(BscHardfork::Bohr).active_at_block(block) {
        BscSpecId::BOHR
    } else if chain_spec.fork(BscHardfork::HaberFix).active_at_block(block) {
        BscSpecId::HABER_FIX
    } else if chain_spec.fork(BscHardfork::Haber).active_at_block(block) {
        BscSpecId::HABER
    } else if chain_spec.fork(EthereumHardfork::Cancun).active_at_block(block) {
        BscSpecId::CANCUN
    } else if chain_spec.fork(BscHardfork::FeynmanFix).active_at_block(block) {
        BscSpecId::FEYNMAN_FIX
    } else if chain_spec.fork(BscHardfork::Feynman).active_at_block(block) {
        BscSpecId::FEYNMAN
    } else if chain_spec.fork(BscHardfork::Kepler).active_at_block(block) {
        BscSpecId::KEPLER
    } else if chain_spec.fork(EthereumHardfork::Shanghai).active_at_block(block) {
        BscSpecId::SHANGHAI
    } else if chain_spec.fork(BscHardfork::HertzFix).active_at_block(block) {
        BscSpecId::HERTZ_FIX
    } else if chain_spec.fork(BscHardfork::Hertz).active_at_block(block) {
        BscSpecId::HERTZ
    } else if chain_spec.fork(EthereumHardfork::London).active_at_block(block) {
        BscSpecId::LONDON
    } else if chain_spec.fork(EthereumHardfork::Berlin).active_at_block(block) {
        BscSpecId::BERLIN
    } else if chain_spec.fork(BscHardfork::Plato).active_at_block(block) {
        BscSpecId::PLATO
    } else if chain_spec.fork(BscHardfork::Luban).active_at_block(block) {
        BscSpecId::LUBAN
    } else if chain_spec.fork(BscHardfork::Planck).active_at_block(block) {
        BscSpecId::PLANCK
    } else if chain_spec.fork(BscHardfork::Gibbs).active_at_block(block) {
        // bsc mainnet and testnet have different order for Moran, Nano and Gibbs
        if chain_spec.fork(BscHardfork::Moran).active_at_block(block) {
            BscSpecId::MORAN
        } else if chain_spec.fork(BscHardfork::Nano).active_at_block(block) {
            BscSpecId::NANO
        } else {
            BscSpecId::EULER
        }
    } else if chain_spec.fork(BscHardfork::Moran).active_at_block(block) {
        BscSpecId::MORAN
    } else if chain_spec.fork(BscHardfork::Nano).active_at_block(block) {
        BscSpecId::NANO
    } else if chain_spec.fork(BscHardfork::Euler).active_at_block(block) {
        BscSpecId::EULER
    } else if chain_spec.fork(BscHardfork::Bruno).active_at_block(block) {
        BscSpecId::BRUNO
    } else if chain_spec.fork(BscHardfork::MirrorSync).active_at_block(block) {
        BscSpecId::MIRROR_SYNC
    } else if chain_spec.fork(BscHardfork::Niels).active_at_block(block) {
        BscSpecId::NIELS
    } else if chain_spec.fork(BscHardfork::Ramanujan).active_at_block(block) {
        BscSpecId::RAMANUJAN
    } else if chain_spec.fork(EthereumHardfork::MuirGlacier).active_at_block(block) {
        BscSpecId::MUIR_GLACIER
    } else if chain_spec.fork(EthereumHardfork::Istanbul).active_at_block(block) {
        BscSpecId::ISTANBUL
    } else if chain_spec.fork(EthereumHardfork::Petersburg).active_at_block(block) {
        BscSpecId::PETERSBURG
    } else if chain_spec.fork(EthereumHardfork::Constantinople).active_at_block(block) {
        BscSpecId::CONSTANTINOPLE
    } else if chain_spec.fork(EthereumHardfork::Byzantium).active_at_block(block) {
        BscSpecId::BYZANTIUM
    } else if chain_spec.fork(EthereumHardfork::Homestead).active_at_block(block) {
        BscSpecId::HOMESTEAD
    } else if chain_spec.fork(EthereumHardfork::Frontier).active_at_block(block) {
        BscSpecId::FRONTIER
    } else {
        panic!(
            "invalid hardfork chainspec: expected at least one hardfork, got {:?}",
            chain_spec.hardforks
        )
    }
}
