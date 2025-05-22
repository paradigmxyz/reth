use crate::evm::{CustomEvmTransaction, CustomTxEnv};
use alloy_evm::{precompiles::PrecompilesMap, Database, Evm, EvmEnv, EvmFactory};
use alloy_op_evm::OpEvm;
use alloy_primitives::{Address, Bytes};
use op_revm::{
    precompiles::OpPrecompiles, DefaultOp, L1BlockInfo, OpBuilder, OpContext, OpHaltReason,
    OpSpecId, OpTransaction, OpTransactionError,
};
use reth_ethereum::evm::revm::{
    context::{result::ResultAndState, BlockEnv, CfgEnv},
    handler::PrecompileProvider,
    interpreter::InterpreterResult,
    Context, Inspector, Journal,
};
use revm::{context_interface::result::EVMError, inspector::NoOpInspector};
use std::error::Error;

/// EVM context contains data that EVM needs for execution of [`CustomEvmTransaction`].
pub type CustomContext<DB> =
    Context<BlockEnv, OpTransaction<CustomTxEnv>, CfgEnv<OpSpecId>, DB, Journal<DB>, L1BlockInfo>;

pub struct CustomEvm<DB: Database, I, P = OpPrecompiles> {
    inner: OpEvm<DB, I, P>,
}

impl<DB, I, P> Evm for CustomEvm<DB, I, P>
where
    DB: Database,
    I: Inspector<OpContext<DB>>,
    P: PrecompileProvider<OpContext<DB>, Output = InterpreterResult>,
{
    type DB = DB;
    type Tx = CustomEvmTransaction;
    type Error = EVMError<DB::Error, OpTransactionError>;
    type HaltReason = OpHaltReason;
    type Spec = OpSpecId;
    type Precompiles = P;
    type Inspector = I;

    fn block(&self) -> &BlockEnv {
        Evm::block(&self.inner)
    }

    fn chain_id(&self) -> u64 {
        Evm::chain_id(&self.inner)
    }

    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        match tx {
            CustomEvmTransaction::Op(tx) => Evm::transact_raw(&mut self.inner, tx),
            CustomEvmTransaction::Payment(..) => todo!(),
        }
    }

    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        Evm::transact_system_call(&mut self.inner, caller, contract, data)
    }

    fn db_mut(&mut self) -> &mut Self::DB {
        Evm::db_mut(&mut self.inner)
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec>) {
        Evm::finish(self.inner)
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        Evm::set_inspector_enabled(&mut self.inner, enabled)
    }

    fn precompiles(&self) -> &Self::Precompiles {
        Evm::precompiles(&self.inner)
    }

    fn precompiles_mut(&mut self) -> &mut Self::Precompiles {
        Evm::precompiles_mut(&mut self.inner)
    }

    fn inspector(&self) -> &Self::Inspector {
        Evm::inspector(&self.inner)
    }

    fn inspector_mut(&mut self) -> &mut Self::Inspector {
        Evm::inspector_mut(&mut self.inner)
    }
}

pub struct CustomEvmFactory;

impl EvmFactory for CustomEvmFactory {
    type Evm<DB: Database, I: Inspector<OpContext<DB>>> = CustomEvm<DB, I, Self::Precompiles>;
    type Context<DB: Database> = OpContext<DB>;
    type Tx = CustomEvmTransaction;
    type Error<DBError: Error + Send + Sync + 'static> = EVMError<DBError, OpTransactionError>;
    type HaltReason = OpHaltReason;
    type Spec = OpSpecId;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv<Self::Spec>,
    ) -> Self::Evm<DB, NoOpInspector> {
        let spec_id = input.cfg_env.spec;
        CustomEvm {
            inner: OpEvm::new(
                Context::op()
                    .with_tx(OpTransaction::default())
                    .with_db(db)
                    .with_block(input.block_env)
                    .with_cfg(input.cfg_env)
                    .build_op_with_inspector(NoOpInspector {})
                    .with_precompiles(PrecompilesMap::from_static(
                        OpPrecompiles::new_with_spec(spec_id).precompiles(),
                    )),
                false,
            ),
        }
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<Self::Spec>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        let spec_id = input.cfg_env.spec;

        CustomEvm {
            inner: OpEvm::new(
                Context::op()
                    .with_tx(OpTransaction::default())
                    .with_db(db)
                    .with_block(input.block_env)
                    .with_cfg(input.cfg_env)
                    .build_op_with_inspector(inspector)
                    .with_precompiles(PrecompilesMap::from_static(
                        OpPrecompiles::new_with_spec(spec_id).precompiles(),
                    )),
                true,
            ),
        }
    }
}
