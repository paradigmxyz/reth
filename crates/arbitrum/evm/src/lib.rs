#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::sync::Arc;

mod config;
pub use config::{ArbBlockAssembler, ArbNextBlockEnvAttributes};

mod build;
pub use build::{ArbBlockExecutionCtx, ArbBlockExecutorFactory};
pub mod execute;

pub mod receipts;
pub use receipts::*;
mod predeploys;
pub use predeploys::*;

#[derive(Debug)]
pub struct ArbEvmConfig<ChainSpec = (), N = (), R = ArbRethReceiptBuilder> {
    pub executor_factory: ArbBlockExecutorFactory<R, ChainSpec>,
    pub block_assembler: ArbBlockAssembler<ChainSpec>,
    _pd: core::marker::PhantomData<N>,
}

impl<ChainSpec, N: Clone, R: Clone> Clone for ArbEvmConfig<ChainSpec, N, R> {
    fn clone(&self) -> Self {
        Self {
            executor_factory: self.executor_factory.clone(),
            block_assembler: self.block_assembler.clone(),
            _pd: self._pd.clone(),
        }
    }
}

impl<ChainSpec, N, R> Default for ArbEvmConfig<ChainSpec, N, R>
where
    ChainSpec: Default,
    R: Default + Clone,
{
    fn default() -> Self {
        let cs = Arc::new(ChainSpec::default());
        Self::new(cs, R::default())
    }
}

impl<ChainSpec, N, R> ArbEvmConfig<ChainSpec, N, R>
where
    R: Clone,
{
    pub fn new(chain_spec: Arc<ChainSpec>, receipt_builder: R) -> Self {
        Self {
            block_assembler: ArbBlockAssembler::new(chain_spec.clone()),
            executor_factory: ArbBlockExecutorFactory::new(receipt_builder, chain_spec),
            _pd: core::marker::PhantomData,
        }
    }

    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        self.executor_factory.spec()
    }
use alloy_consensus::Header;
use alloy_primitives::U256;
use reth_evm::{ConfigureEvm, EvmEnv};
use revm::{
    context::{BlockEnv, CfgEnv},
    primitives::hardfork::SpecId,
};

impl<ChainSpec, N, R> ConfigureEvm for ArbEvmConfig<ChainSpec, N, R> {
    type Primitives = N;
    type Error = core::convert::Infallible;
    type NextBlockEnvCtx = ArbNextBlockEnvAttributes;
    type BlockExecutorFactory = ArbBlockExecutorFactory<R, ChainSpec>;
    type BlockAssembler = ArbBlockAssembler<ChainSpec>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn evm_env(&self, _header: &Header) -> EvmEnv<SpecId> {
        let cfg_env = CfgEnv::new().with_chain_id(0).with_spec(SpecId::LATEST);
        let block_env = BlockEnv {
            number: U256::ZERO,
            beneficiary: Default::default(),
            timestamp: U256::ZERO,
            difficulty: U256::ZERO,
            prevrandao: None,
            gas_limit: 0,
            basefee: 0,
            blob_excess_gas_and_price: None,
        };
        EvmEnv { cfg_env, block_env }
    }

    fn next_evm_env(
        &self,
        _parent: &Header,
        _attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnv<SpecId>, Self::Error> {
        let cfg_env = CfgEnv::new().with_chain_id(0).with_spec(SpecId::LATEST);
        let block_env = BlockEnv {
            number: U256::ZERO,
            beneficiary: Default::default(),
            timestamp: U256::ZERO,
            difficulty: U256::ZERO,
            prevrandao: None,
            gas_limit: 0,
            basefee: 0,
            blob_excess_gas_and_price: None,
        };
        Ok(EvmEnv { cfg_env, block_env })
    }
}
use reth_evm::{ConfigureEngineEvm, EvmEnvFor, ExecutableTxIterator, ExecutionCtxFor};
use reth_primitives_traits::{TxTy, WithEncoded};
use reth_storage_errors::any::AnyError;
use reth_arbitrum_payload::ArbExecutionData;

impl<ChainSpec, N, R> ConfigureEngineEvm<ArbExecutionData> for ArbEvmConfig<ChainSpec, N, R>
{
    fn evm_env_for_payload(
        &self,
        payload: &ArbExecutionData,
    ) -> EvmEnvFor<Self> {
        let cfg_env = CfgEnv::new().with_chain_id(0).with_spec(SpecId::LATEST);
        let block_env = BlockEnv {
            number: U256::from(payload.payload.block_number()),
            beneficiary: payload.payload.as_v1().fee_recipient,
            timestamp: U256::from(payload.payload.timestamp()),
            difficulty: U256::ZERO,
            prevrandao: Some(payload.payload.as_v1().prev_randao),
            gas_limit: payload.payload.as_v1().gas_limit,
            basefee: payload.payload.as_v1().base_fee_per_gas.to(),
            blob_excess_gas_and_price: None,
        };
        EvmEnv { cfg_env, block_env }
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a ArbExecutionData,
    ) -> ExecutionCtxFor<'a, Self> {
        ArbBlockExecutionCtx {
            parent_hash: payload.parent_hash(),
            parent_beacon_block_root: payload.sidecar.parent_beacon_block_root,
            extra_data: payload.payload.as_v1().extra_data.clone(),
        }
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &ArbExecutionData,
    ) -> impl ExecutableTxIterator<Self> {
        payload
            .payload
            .transactions()
            .clone()
            .into_iter()
            .map(|encoded| {
                let tx = TxTy::<Self::Primitives>::decode_2718_exact(encoded.as_ref())
                    .map_err(AnyError::new)?;
                let signer = tx.try_recover().map_err(AnyError::new)?;
                Ok::<_, AnyError>(WithEncoded::new(encoded, tx.with_signer(signer)))
            })
    }
}


}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arb_evm_config_default_constructs() {
        let _cfg = ArbEvmConfig::<(), (), ArbRethReceiptBuilder>::default();
    }

    #[test]
    fn arb_block_assembler_and_factory_construct() {
        let cs = alloc::sync::Arc::new(());
        let _asm = ArbBlockAssembler::new(cs.clone());
        let _fac = ArbBlockExecutorFactory::new(ArbRethReceiptBuilder, cs);
    }
}
