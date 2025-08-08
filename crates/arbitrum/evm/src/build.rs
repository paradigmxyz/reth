#![allow(unused)]
extern crate alloc;

use alloc::{boxed::Box, sync::Arc};
use alloy_evm::{eth::EthEvmContext, precompiles::PrecompilesMap, EvmFactory};
use alloy_primitives::{address, Address, Bytes, U256, B256};
use reth_evm_ethereum::{
    EthEvm, EthEvmConfig,
    revm::{
        context::{Context, TxEnv},
        handler::EthPrecompiles,
        inspector::{Inspector, NoOpInspector},
        interpreter::interpreter::EthInterpreter,
        precompile::{PrecompileFn, PrecompileOutput, PrecompileResult},
        primitives::hardfork::SpecId,
    },
};
use reth_evm::{
    block::{
        builder::{BlockAssembler, BlockBuilder},
        executor::{BasicBlockExecutor, BlockExecutionError},
    },
    evm::{ConfigureEvm, EvmEnv, EvmFor, InspectorFor},
    primitives::{Block as PrimitivesBlock, Database, NodePrimitives, SealedBlock, SealedHeader, State},
    ContextInterface, ContextProviders, State as _,
};
use alloy_evm::block::{BlockExecutorFactory, BlockExecutorFor, CommitChanges, ExecutableTx, BlockExecutor as AlloyBlockExecutor};
use alloy_evm::eth::{EthBlockExecutionCtx, EthBlockExecutor};
use reth_evm::execute::BlockExecutionResult as RethBlockExecutionResult;
use reth_evm::OnStateHook;
use reth_evm_ethereum::revm::context::result::ExecutionResult;

use crate::predeploys::{PredeployCallContext, PredeployRegistry};

#[derive(Debug, Clone)]
pub struct ArbBlockExecutorFactory<R, CS> {
    receipt_builder: R,
    spec: Arc<CS>,
    predeploys: Arc<PredeployRegistry>,
    evm_factory: ArbEvmFactory,
}

#[derive(Debug, Clone, Default)]
pub struct ArbBlockExecutionCtx {
    pub parent_hash: B256,
    pub parent_beacon_block_root: Option<B256>,
    pub extra_data: Bytes,
}

pub struct ArbBlockExecutor<'a, Evm, CS, RB> {
    inner: EthBlockExecutor<'a, Evm, &'a Arc<CS>, &'a RB>,
}

impl<R: Clone, CS> ArbBlockExecutorFactory<R, CS> {
    pub fn new(receipt_builder: R, spec: Arc<CS>) -> Self {
        let predeploys = Arc::new(PredeployRegistry::with_default_addresses());
        let evm_factory = ArbEvmFactory { predeploys: predeploys.clone() };
        Self { receipt_builder, spec, predeploys, evm_factory }
    }

    pub const fn spec(&self) -> &Arc<CS> {
        &self.spec
    }

    pub fn evm_factory(&self) -> ArbEvmFactory {
        ArbEvmFactory { predeploys: self.predeploys.clone() }
    }
}

impl<'db, DB, E, CS, RB> AlloyBlockExecutor for ArbBlockExecutor<'_, E, CS, RB>
where
    DB: Database + 'db,
    E: reth_evm::Evm<DB = &'db mut State<DB>, Tx = TxEnv>,
{
    type Transaction = reth_arbitrum_primitives::ArbTransactionSigned;
    type Receipt = reth_arbitrum_primitives::ArbReceipt;
    type Evm = E;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.inner.apply_pre_execution_changes()
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutableTx<Self>,
        f: impl FnOnce(&ExecutionResult<<Self::Evm as reth_evm::Evm>::HaltReason>) -> CommitChanges,
    ) -> Result<Option<u64>, BlockExecutionError> {
        self.inner.execute_transaction_with_commit_condition(tx, f)
    }

    fn finish(self) -> Result<(Self::Evm, RethBlockExecutionResult<reth_arbitrum_primitives::ArbReceipt>), BlockExecutionError> {
        self.inner.finish()
    }

    fn set_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.inner.set_state_hook(hook)
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
    }

    fn evm(&self) -> &Self::Evm {
        self.inner.evm()
    }
}
 
#[derive(Debug, Clone, Default)]
pub struct ArbEvmFactory {
    predeploys: Arc<PredeployRegistry>,
}

impl EvmFactory for ArbEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>, EthInterpreter>> =
        EthEvm<DB, I, PrecompilesMap>;
    type Tx = TxEnv;
    type Error<DBError: core::error::Error + Send + Sync + 'static> =
        reth_evm_ethereum::revm::context_interface::result::EVMError<DBError>;
    type HaltReason = reth_evm_ethereum::revm::context_interface::result::HaltReason;
    type Context<DB: Database> = EthEvmContext<DB>;
    type Spec = SpecId;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(&self, db: DB, input: EvmEnv) -> Self::Evm<DB, NoOpInspector> {
        let mut evm = Context::mainnet()
            .with_db(db)
            .with_cfg(input.cfg_env)
            .with_block(input.block_env)
            .build_mainnet_with_inspector(NoOpInspector {})
            .with_precompiles(PrecompilesMap::from_static(EthPrecompiles::default().precompiles));

        let mut custom = PrecompilesMap::default();
        let reg = self.predeploys.clone();

        fn mk_handler(
            reg: Arc<PredeployRegistry>,
            addr: Address,
        ) -> (Address, PrecompileFn) {
            let f = move |input: &[u8], ctx: &mut EthEvmContext<_>| -> PrecompileResult {
                let gas_limit = ctx.env.tx().gas_limit();
                let value = U256::from(ctx.env.tx().caller_value());
                let block = &ctx.env.block;
                let cfg = &ctx.env.cfg;

                let call_ctx = PredeployCallContext {
                    block_number: block.number.to(),
                    block_hashes: alloc::vec::Vec::new(),
                    chain_id: U256::from(cfg.chain_id),
                    os_version: 0,
                    time: block.timestamp.to(),
                    origin: ctx.env.tx().caller(),
                    caller: ctx.env.tx().caller(),
                    depth: ctx.depth as u64,
                };
                let bytes = Bytes::copy_from_slice(input);
                let (ret, gas_left, success) =
                    reg.dispatch(&call_ctx, addr, &bytes, gas_limit, value).unwrap_or_default();
                let out = PrecompileOutput::new(gas_left, ret);
                if success {
                    PrecompileResult::Ok(out)
                } else {
                    PrecompileResult::Error { exit_status: Default::default(), output: out }
                }
            };
            (addr, f as PrecompileFn)
        }

        let sys = address!("0000000000000000000000000000000000000064");
        let retry = address!("000000000000000000000000000000000000006e");
        let owner = address!("0000000000000000000000000000000000000070");
        let atab = address!("0000000000000000000000000000000000000066");

        custom.extend([mk_handler(reg.clone(), sys), mk_handler(reg.clone(), retry), mk_handler(reg.clone(), owner), mk_handler(reg, atab)]);

        evm = evm.with_precompiles(custom);
        EthEvm::new(evm, false)
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>, EthInterpreter>>(
        &self,
        db: DB,
        input: EvmEnv,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        EthEvm::new(self.create_evm(db, input).into_inner().with_inspector(inspector), true)
    }
}

impl<R: Clone, CS> BlockExecutorFactory for ArbBlockExecutorFactory<R, CS> {
    type EvmFactory = ArbEvmFactory;
    type ExecutionCtx<'a> = ArbBlockExecutionCtx;
    type Transaction = reth_arbitrum_primitives::ArbTransactionSigned;
    type Receipt = reth_arbitrum_primitives::ArbReceipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.evm_factory
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: EthEvm<&'a mut State<DB>, I, PrecompilesMap>,
        ctx: ArbBlockExecutionCtx,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: Database + 'a,
        I: InspectorFor<Self, &'a mut State<DB>> + 'a,
    {
        let eth_ctx: EthBlockExecutionCtx<'a> = EthBlockExecutionCtx {
            parent_hash: ctx.parent_hash,
            parent_beacon_block_root: ctx.parent_beacon_block_root,
            extra_data: ctx.extra_data,
            ..Default::default()
        };
        ArbBlockExecutor {
            inner: EthBlockExecutor::new(
                evm,
                eth_ctx,
                &self.spec,
                &self.receipt_builder,
            ),
        }
    }
}
