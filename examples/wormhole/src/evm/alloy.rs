use crate::evm::WormholeTxEnv;
use alloy_evm::{precompiles::PrecompilesMap, Database, Evm, EvmEnv, EvmFactory};
use alloy_op_evm::{OpEvm, OpEvmFactory};
use alloy_primitives::{Address, Bytes};
use op_revm::{
    precompiles::OpPrecompiles, L1BlockInfo, OpContext, OpHaltReason, OpSpecId, OpTransaction,
    OpTransactionError,
};
use revm::{
    context::{result::ResultAndState, BlockEnv, CfgEnv},
    context_interface::result::EVMError,
    handler::PrecompileProvider,
    inspector::NoOpInspector,
    interpreter::InterpreterResult,
    Context, Inspector, Journal,
};
use std::error::Error;

/// EVM context contains data that EVM needs for execution of [`WormholeTxEnv`].
pub type WormholeContext<DB> =
    Context<BlockEnv, OpTransaction<WormholeTxEnv>, CfgEnv<OpSpecId>, DB, Journal<DB>, L1BlockInfo>;

pub struct WormholeEvm<DB: Database, I, P = OpPrecompiles> {
    inner: OpEvm<DB, I, P>,
}

impl<DB: Database, I, P> WormholeEvm<DB, I, P> {
    pub fn new(op: OpEvm<DB, I, P>) -> Self {
        Self { inner: op }
    }
}

impl<DB, I, P> Evm for WormholeEvm<DB, I, P>
where
    DB: Database,
    I: Inspector<OpContext<DB>>,
    P: PrecompileProvider<OpContext<DB>, Output = InterpreterResult>,
{
    type DB = DB;
    type Tx = WormholeTxEnv;
    type Error = EVMError<DB::Error, OpTransactionError>;
    type HaltReason = OpHaltReason;
    type Spec = OpSpecId;
    type Precompiles = P;
    type Inspector = I;

    fn block(&self) -> &BlockEnv {
        self.inner.block()
    }

    fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }

    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        match tx {
            WormholeTxEnv::Op(tx) => self.inner.transact_raw(tx),
            WormholeTxEnv::Wormhole(_tx) => {
                // TODO: Implement custom Wormhole transaction execution
                // For now, this is a placeholder that would contain:
                // 1. Wormhole-specific validation
                // 2. Cross-chain message verification
                // 3. Custom state transitions
                // 4. Special gas accounting

                // For demo purposes, we'll return an error indicating not implemented
                Err(EVMError::Transaction(OpTransactionError::Base(
                    revm::context_interface::result::InvalidTransaction::InvalidChainId,
                )))
            }
        }
    }

    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        self.inner.transact_system_call(caller, contract, data)
    }

    fn db_mut(&mut self) -> &mut Self::DB {
        self.inner.db_mut()
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec>) {
        self.inner.finish()
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.inner.set_inspector_enabled(enabled)
    }

    fn precompiles(&self) -> &Self::Precompiles {
        self.inner.precompiles()
    }

    fn precompiles_mut(&mut self) -> &mut Self::Precompiles {
        self.inner.precompiles_mut()
    }

    fn inspector(&self) -> &Self::Inspector {
        self.inner.inspector()
    }

    fn inspector_mut(&mut self) -> &mut Self::Inspector {
        self.inner.inspector_mut()
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct WormholeEvmFactory(pub OpEvmFactory);

impl WormholeEvmFactory {
    pub fn new() -> Self {
        Self::default()
    }
}

impl EvmFactory for WormholeEvmFactory {
    type Evm<DB: Database, I: Inspector<OpContext<DB>>> = WormholeEvm<DB, I, Self::Precompiles>;
    type Context<DB: Database> = OpContext<DB>;
    type Tx = WormholeTxEnv;
    type Error<DBError: Error + Send + Sync + 'static> = EVMError<DBError, OpTransactionError>;
    type HaltReason = OpHaltReason;
    type Spec = OpSpecId;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv<Self::Spec>,
    ) -> Self::Evm<DB, NoOpInspector> {
        WormholeEvm::new(self.0.create_evm(db, input))
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<Self::Spec>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        WormholeEvm::new(self.0.create_evm_with_inspector(db, input, inspector))
    }
}
