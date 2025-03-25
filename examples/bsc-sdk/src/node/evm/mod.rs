//! EVM config for bsc.
use crate::{
    chainspec::BscChainSpec,
    evm::{
        api::{
            builder::BscBuilder,
            ctx::{BscContext, DefaultBsc},
            BscEvm,
        },
        spec::BscSpecId,
        transaction::BscTransaction,
    },
};
use alloy_primitives::Bytes;
use std::sync::Arc;

use reth_evm::{
    block::{BlockExecutorFactory, BlockExecutorFor},
    eth::{EthBlockExecutionCtx, EthBlockExecutor, EthBlockExecutorFactory},
    EvmEnv, EvmFactory, InspectorFor,
};
use reth_evm_ethereum::{EthBlockAssembler, RethReceiptBuilder};
use reth_primitives::{Receipt, TransactionSigned};
use reth_revm::{Context, Database, State};
use revm::{
    context::{
        result::{EVMError, HaltReason},
        TxEnv,
    },
    inspector::NoOpInspector,
    Inspector,
};

mod config;

/// Factory producing [`EthEvm`].
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct BscEvmFactory;

impl EvmFactory for BscEvmFactory {
    type Evm<DB: Database<Error: Send + Sync + 'static>, I: Inspector<BscContext<DB>>> =
        BscEvm<DB, I>;
    type Tx = BscTransaction<TxEnv>;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Context<DB: Database<Error: Send + Sync + 'static>> = BscContext<DB>;
    type Spec = BscSpecId;

    fn create_evm<DB: Database<Error: Send + Sync + 'static>>(
        &self,
        db: DB,
        input: EvmEnv<BscSpecId>,
    ) -> Self::Evm<DB, NoOpInspector> {
        BscEvm {
            inner: Context::bsc()
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .with_db(db)
                .build_bsc_with_inspector(NoOpInspector {}),
            inspect: false,
        }
    }

    fn create_evm_with_inspector<
        DB: Database<Error: Send + Sync + 'static>,
        I: Inspector<Self::Context<DB>>,
    >(
        &self,
        db: DB,
        input: EvmEnv<BscSpecId>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        BscEvm {
            inner: Context::bsc()
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .with_db(db)
                .build_bsc_with_inspector(inspector),
            inspect: true,
        }
    }
}

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
    {
        EthBlockExecutor::new(evm, ctx, self.chain_spec(), self.executor_factory.receipt_builder())
    }
}

// impl<ChainSpec, N, R> ConfigureEvm for OpEvmConfig<ChainSpec, N, R>
// where
//     ChainSpec: EthChainSpec + OpHardforks,
//     N: NodePrimitives<
//         Receipt = R::Receipt,
//         SignedTx = R::Transaction,
//         BlockHeader = Header,
//         BlockBody = alloy_consensus::BlockBody<R::Transaction>,
//         Block = alloy_consensus::Block<R::Transaction>,
//     >,
//     OpTransaction<TxEnv>: FromRecoveredTx<N::SignedTx>,
//     R: OpReceiptBuilder<Receipt: DepositReceipt, Transaction: SignedTransaction>,
//     Self: Send + Sync + Unpin + Clone + 'static,
// {
//     type Primitives = N;
//     type Error = EIP1559ParamError;
//     type NextBlockEnvCtx = OpNextBlockEnvAttributes;
//     type BlockExecutorFactory = OpBlockExecutorFactory<R, Arc<ChainSpec>>;
//     type BlockAssembler = OpBlockAssembler<ChainSpec>;

//     fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
//         &self.executor_factory
//     }

//     fn block_assembler(&self) -> &Self::BlockAssembler {
//         &self.block_assembler
//     }

//     fn evm_env(&self, header: &Header) -> EvmEnv<OpSpecId> {
//         let spec = config::revm_spec(self.chain_spec(), header);

//         let cfg_env =
// CfgEnv::new().with_chain_id(self.chain_spec().chain().id()).with_spec(spec);

//         let block_env = BlockEnv {
//             number: header.number(),
//             beneficiary: header.beneficiary(),
//             timestamp: header.timestamp(),
//             difficulty: if spec.into_eth_spec() >= SpecId::MERGE {
//                 U256::ZERO
//             } else {
//                 header.difficulty()
//             },
//             prevrandao: if spec.into_eth_spec() >= SpecId::MERGE {
//                 header.mix_hash()
//             } else {
//                 None
//             },
//             gas_limit: header.gas_limit(),
//             basefee: header.base_fee_per_gas().unwrap_or_default(),
//             // EIP-4844 excess blob gas of this block, introduced in Cancun
//             blob_excess_gas_and_price: header.excess_blob_gas().map(|excess_blob_gas| {
//                 BlobExcessGasAndPrice::new(excess_blob_gas, spec.into_eth_spec() >=
// SpecId::PRAGUE)             }),
//         };

//         EvmEnv { cfg_env, block_env }
//     }

//     fn next_evm_env(
//         &self,
//         parent: &Header,
//         attributes: &Self::NextBlockEnvCtx,
//     ) -> Result<EvmEnv<OpSpecId>, Self::Error> {
//         // ensure we're not missing any timestamp based hardforks
//         let spec_id = revm_spec_by_timestamp_after_bedrock(self.chain_spec(),
// attributes.timestamp);

//         // configure evm env based on parent block
//         let cfg_env =
//             CfgEnv::new().with_chain_id(self.chain_spec().chain().id()).with_spec(spec_id);

//         // if the parent block did not have excess blob gas (i.e. it was pre-cancun), but it is
//         // cancun now, we need to set the excess blob gas to the default value(0)
//         let blob_excess_gas_and_price = parent
//             .maybe_next_block_excess_blob_gas(
//                 self.chain_spec().blob_params_at_timestamp(attributes.timestamp),
//             )
//             .or_else(|| (spec_id.into_eth_spec().is_enabled_in(SpecId::CANCUN)).then_some(0))
//             .map(|gas| BlobExcessGasAndPrice::new(gas, false));

//         let block_env = BlockEnv {
//             number: parent.number() + 1,
//             beneficiary: attributes.suggested_fee_recipient,
//             timestamp: attributes.timestamp,
//             difficulty: U256::ZERO,
//             prevrandao: Some(attributes.prev_randao),
//             gas_limit: attributes.gas_limit,
//             // calculate basefee based on parent block's gas usage
//             basefee: next_block_base_fee(self.chain_spec(), parent, attributes.timestamp)?,
//             // calculate excess gas based on parent block's blob gas usage
//             blob_excess_gas_and_price,
//         };

//         Ok(EvmEnv { cfg_env, block_env })
//     }

//     fn context_for_block(&self, block: &'_ SealedBlock<N::Block>) -> OpBlockExecutionCtx {
//         OpBlockExecutionCtx {
//             parent_hash: block.header().parent_hash(),
//             parent_beacon_block_root: block.header().parent_beacon_block_root(),
//             extra_data: block.header().extra_data().clone(),
//         }
//     }

//     fn context_for_next_block(
//         &self,
//         parent: &SealedHeader<N::BlockHeader>,
//         attributes: Self::NextBlockEnvCtx,
//     ) -> OpBlockExecutionCtx {
//         OpBlockExecutionCtx {
//             parent_hash: parent.hash(),
//             parent_beacon_block_root: attributes.parent_beacon_block_root,
//             extra_data: attributes.extra_data,
//         }
//     }
// }

// impl ConfigureEvmEnv for BscEvmConfig {
//     type Header = Header;
//     type Error = Infallible; // TODO: error type

//     fn fill_tx_env(&self, tx_env: &mut TxEnv, transaction: &TransactionSigned, sender: Address) {
//         transaction.fill_tx_env(tx_env, sender);
//     }

//     fn fill_tx_env_system_contract_call(
//         &self,
//         _env: &mut Env,
//         _caller: Address,
//         _contract: Address,
//         _data: Bytes,
//     ) {
//         // No system contract call on BSC
//     }

//     fn fill_cfg_env(
//         &self,
//         cfg_env: &mut CfgEnvWithHandlerCfg,
//         header: &Header,
//         total_difficulty: U256,
//     ) {
//         let spec_id = revm_spec(
//             self.chain_spec(),
//             &Head {
//                 number: header.number,
//                 timestamp: header.timestamp,
//                 difficulty: header.difficulty,
//                 total_difficulty,
//                 hash: Default::default(),
//             },
//         );

//         cfg_env.chain_id = self.chain_spec.chain().id();
//         cfg_env.perf_analyse_created_bytecodes = AnalysisKind::Analyse;

//         // Disable block gas limit check
//         // system transactions do not have gas limit
//         cfg_env.disable_block_gas_limit = true;

//         cfg_env.handler_cfg.spec_id = spec_id;
//         cfg_env.handler_cfg.is_bsc = self.chain_spec.is_bsc();
//     }

//     fn next_cfg_and_block_env(
//         &self,
//         parent: &Self::Header,
//         attributes: NextBlockEnvAttributes,
//     ) -> Result<(CfgEnvWithHandlerCfg, BlockEnv), Self::Error> {
//         // configure evm env based on parent block
//         let cfg = CfgEnv::default().with_chain_id(self.chain_spec.chain().id());

//         // ensure we're not missing any timestamp based hardforks
//         let spec_id = revm_spec_by_timestamp_after_shanghai(&self.chain_spec,
// attributes.timestamp);

//         // if the parent block did not have excess blob gas (i.e. it was pre-cancun), but it is
//         // cancun now, we need to set the excess blob gas to the default value
//         let blob_excess_gas_and_price = parent
//             .next_block_excess_blob_gas()
//             .or_else(|| (spec_id == SpecId::CANCUN).then_some(0))
//             .map(BlobExcessGasAndPrice::new);

//         let mut basefee = parent.next_block_base_fee(
//             self.chain_spec.base_fee_params_at_timestamp(attributes.timestamp),
//         );

//         let mut gas_limit = U256::from(parent.gas_limit);

//         // If we are on the London fork boundary, we need to multiply the parent's gas limit by
//         // the elasticity multiplier to get the new gas limit.
//         if self.chain_spec.fork(EthereumHardfork::London).transitions_at_block(parent.number + 1)
// {             let elasticity_multiplier = self
//                 .chain_spec
//                 .base_fee_params_at_timestamp(attributes.timestamp)
//                 .elasticity_multiplier;

//             // multiply the gas limit by the elasticity multiplier
//             gas_limit *= U256::from(elasticity_multiplier);

//             // set the base fee to the initial base fee from the EIP-1559 spec
//             basefee = Some(EIP1559_INITIAL_BASE_FEE)
//         }

//         let block_env = BlockEnv {
//             number: U256::from(parent.number + 1),
//             coinbase: attributes.suggested_fee_recipient,
//             timestamp: U256::from(attributes.timestamp),
//             difficulty: U256::ZERO,
//             prevrandao: Some(attributes.prev_randao),
//             gas_limit,
//             // calculate basefee based on parent block's gas usage
//             basefee: basefee.map(U256::from).unwrap_or_default(),
//             // calculate excess gas based on parent block's blob gas usage
//             blob_excess_gas_and_price,
//         };

//         Ok((CfgEnvWithHandlerCfg::new_with_spec_id(cfg, spec_id), block_env))
//     }
// }

// impl ConfigureEvm for BscEvmConfig {
//     type DefaultExternalContext<'a> = ();

//     fn evm<DB: Database>(&self, db: DB) -> Evm<'_, Self::DefaultExternalContext<'_>, DB> {
//         EvmBuilder::default().with_db(db).bsc().build()
//     }

//     fn evm_with_inspector<DB, I>(&self, db: DB, inspector: I) -> Evm<'_, I, DB>
//     where
//         DB: Database,
//         I: GetInspector<DB>,
//     {
//         EvmBuilder::default()
//             .with_db(db)
//             .with_external_context(inspector)
//             .bsc()
//             .append_handler_register(inspector_handle_register)
//             .build()
//     }

//     fn default_external_context<'a>(&self) -> Self::DefaultExternalContext<'a> {}
// }

// #[cfg(test)]
// mod tests {
//     use alloy_genesis::Genesis;
//     use reth_chainspec::{Chain, ChainSpec};
//     use reth_primitives::revm_primitives::{BlockEnv, CfgEnv};
//     use revm_primitives::SpecId;

//     use super::*;

//     #[test]
//     #[ignore]
//     fn test_fill_cfg_and_block_env() {
//         let mut cfg_env = CfgEnvWithHandlerCfg::new_with_spec_id(CfgEnv::default(),
// SpecId::LATEST);         let mut block_env = BlockEnv::default();
//         let header = Header::default();
//         let total_difficulty = U256::ZERO;

//         let chain_spec = ChainSpec::builder()
//             .chain(Chain::bsc_mainnet())
//             .genesis(Genesis::default())
//             .london_activated()
//             .paris_activated()
//             .shanghai_activated()
//             .build();

//         BscEvmConfig::new(Arc::new(BscChainSpec { inner: chain_spec.clone() }))
//             .fill_cfg_and_block_env(&mut cfg_env, &mut block_env, &header, total_difficulty);

//         assert_eq!(cfg_env.chain_id, chain_spec.chain().id());
//     }
// }
