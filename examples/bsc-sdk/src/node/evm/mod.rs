use crate::evm::{
    api::{ctx::BscContext, BscEvmInner},
    precompiles::BscPrecompiles,
    spec::BscSpecId,
    transaction::BscTransaction,
};
use alloy_primitives::{Address, Bytes};
use reth_evm::{Evm, EvmEnv};
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

mod config;
mod factory;

/// OP EVM implementation.
///
/// This is a wrapper type around the `revm` evm with optional [`Inspector`] (tracing)
/// support. [`Inspector`] support is configurable at runtime because it's part of the underlying
/// [`OpEvm`](op_revm::OpEvm) type.
#[allow(missing_debug_implementations)] // missing revm::OpContext Debug impl
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

    /// Provides a mutable reference to the EVM inspector.
    pub fn inspector_mut(&mut self) -> &mut I {
        &mut self.inner.0.data.inspector
    }
}

impl<DB: Database, I, P> BscEvm<DB, I, P> {
    /// Creates a new OP EVM instance.
    ///
    /// The `inspect` argument determines whether the configured [`Inspector`] of the given
    /// [`OpEvm`](op_revm::OpEvm) should be invoked on [`Evm::transact`].
    pub const fn new(
        evm: BscEvmInner<BscContext<DB>, I, EthInstructions<EthInterpreter, BscContext<DB>>, P>,
        inspect: bool,
    ) -> Self {
        Self { inner: evm, inspect }
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
    type Tx = BscTransaction<TxEnv>;
    type Error = EVMError<DB::Error>;
    type HaltReason = HaltReason;
    type Spec = BscSpecId;

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
        // no system calls on BSC
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
