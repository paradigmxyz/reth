use crate::{
    chainspec::BscChainSpec,
    evm::{
        api::{ctx::BscContext, BscEvmInner},
        precompiles::BscPrecompiles,
        spec::BscSpecId,
        transaction::{BscTxEnv, BscTxTr},
    },
};
use alloy_primitives::{Address, Bytes};
use config::BscEvmConfig;
use reth::{
    api::{FullNodeTypes, NodeTypes},
    builder::{components::ExecutorBuilder, BuilderContext},
};
use reth_evm::{Evm, EvmEnv};
use reth_node_ethereum::BasicBlockExecutorProvider;
use reth_primitives::EthPrimitives;
use revm::{
    context::{
        result::{EVMError, HaltReason, ResultAndState},
        BlockEnv, TxEnv,
    },
    handler::{instructions::EthInstructions, PrecompileProvider},
    interpreter::{interpreter::EthInterpreter, InterpreterResult},
    Context, Database, ExecuteEvm, InspectEvm, Inspector,
};
use std::ops::{Deref, DerefMut};

pub mod config;
mod executor;
mod factory;
mod patch;

/// BSC EVM implementation.
///
/// This is a wrapper type around the `revm` evm with optional [`Inspector`] (tracing)
/// support. [`Inspector`] support is configurable at runtime because it's part of the underlying
#[allow(missing_debug_implementations)]
pub struct BscEvm<DB: Database, I, P = BscPrecompiles> {
    pub inner: BscEvmInner<BscContext<DB>, I, EthInstructions<EthInterpreter, BscContext<DB>>, P>,
    pub inspect: bool,
}

impl<DB: Database, I, P> BscEvm<DB, I, P> {
    /// Provides a reference to the EVM context.
    pub const fn ctx(&self) -> &BscContext<DB> {
        &self.inner.0.data.ctx
    }

    /// Provides a mutable reference to the EVM context.
    pub fn ctx_mut(&mut self) -> &mut BscContext<DB> {
        &mut self.inner.0.data.ctx
    }
}

impl<DB: Database, I, P> Deref for BscEvm<DB, I, P> {
    type Target = BscContext<DB>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.ctx()
    }
}

impl<DB: Database, I, P> DerefMut for BscEvm<DB, I, P> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ctx_mut()
    }
}

impl<DB, I, P> Evm for BscEvm<DB, I, P>
where
    DB: Database,
    I: Inspector<BscContext<DB>>,
    P: PrecompileProvider<BscContext<DB>, Output = InterpreterResult>,
    <DB as revm::Database>::Error: std::marker::Send + std::marker::Sync + 'static,
{
    type DB = DB;
    type Tx = BscTxEnv<TxEnv>;
    type Error = EVMError<DB::Error>;
    type HaltReason = HaltReason;
    type Spec = BscSpecId;

    fn chain_id(&self) -> u64 {
        self.cfg.chain_id
    }

    fn block(&self) -> &BlockEnv {
        &self.block
    }

    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        if self.inspect {
            self.inner.set_tx(tx);
            self.inner.inspect_replay()
        } else if tx.is_system_transaction() {
            let mut gas_limit = tx.base.gas_limit;
            let mut basefee = 0;
            let mut disable_nonce_check = true;

            // ensure the block gas limit is >= the tx
            core::mem::swap(&mut self.block.gas_limit, &mut gas_limit);
            // disable the base fee check for this call by setting the base fee to zero
            core::mem::swap(&mut self.block.basefee, &mut basefee);
            // disable the nonce check
            core::mem::swap(&mut self.cfg.disable_nonce_check, &mut disable_nonce_check);
            let res = self.inner.transact(tx);

            // swap back to the previous gas limit
            core::mem::swap(&mut self.block.gas_limit, &mut gas_limit);
            // swap back to the previous base fee
            core::mem::swap(&mut self.block.basefee, &mut basefee);
            // swap back to the previous nonce check flag
            core::mem::swap(&mut self.cfg.disable_nonce_check, &mut disable_nonce_check);
            return res;
        } else {
            self.inner.transact(tx)
        }
    }

    fn transact_system_call(
        &mut self,
        _caller: Address,
        _contract: Address,
        _data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        unimplemented!()
    }

    fn db_mut(&mut self) -> &mut Self::DB {
        &mut self.journaled_state.database
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec>) {
        let Context { block: block_env, cfg: cfg_env, journaled_state, .. } = self.inner.0.data.ctx;

        (journaled_state.database, EvmEnv { block_env, cfg_env })
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.inspect = enabled;
    }
}

/// A regular bsc evm and executor builder.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct BscExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for BscExecutorBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = BscChainSpec, Primitives = EthPrimitives>>,
{
    type EVM = BscEvmConfig;
    type Executor = BasicBlockExecutorProvider<Self::EVM>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let evm_config = BscEvmConfig::bsc(ctx.chain_spec());
        let executor = BasicBlockExecutorProvider::new(evm_config.clone());

        Ok((evm_config, executor))
    }
}
