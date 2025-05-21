use crate::evm::{CustomEvmTransaction, CustomTxEnv};
use alloy_evm::{precompiles::PrecompilesMap, Database, Evm, EvmEnv, EvmFactory};
use alloy_primitives::{Address, Bytes, TxKind, U256};
use op_alloy_consensus::OpTxType;
use op_revm::{
    precompiles::OpPrecompiles, transaction::deposit::DepositTransactionParts, DefaultOp,
    L1BlockInfo, OpBuilder, OpHaltReason, OpSpecId, OpTransactionError,
};
use reth_ethereum::evm::revm::{
    context::{result::ResultAndState, BlockEnv, CfgEnv, TxEnv},
    handler::{instructions::EthInstructions, PrecompileProvider},
    interpreter::{interpreter::EthInterpreter, InterpreterResult},
    Context, Inspector, Journal,
};
use revm::{
    context_interface::result::EVMError, handler::EvmTr, inspector::NoOpInspector, ExecuteEvm,
    InspectEvm,
};
use std::error::Error;

/// EVM context contains data that EVM needs for execution of [`CustomEvmTransaction`].
pub type CustomContext<DB> =
    Context<BlockEnv, CustomEvmTransaction, CfgEnv<OpSpecId>, DB, Journal<DB>, L1BlockInfo>;

pub struct CustomEvm<DB: Database, I, P = OpPrecompiles> {
    inner:
        op_revm::OpEvm<CustomContext<DB>, I, EthInstructions<EthInterpreter, CustomContext<DB>>, P>,
    inspect: bool,
}

impl<DB, I, P> Evm for CustomEvm<DB, I, P>
where
    DB: Database,
    I: Inspector<CustomContext<DB>>,
    P: PrecompileProvider<CustomContext<DB>, Output = InterpreterResult>,
{
    type DB = DB;
    type Tx = CustomEvmTransaction;
    type Error = EVMError<DB::Error, OpTransactionError>;
    type HaltReason = OpHaltReason;
    type Spec = OpSpecId;
    type Precompiles = P;
    type Inspector = I;

    fn block(&self) -> &BlockEnv {
        &self.inner.ctx_ref().block
    }

    fn chain_id(&self) -> u64 {
        self.inner.ctx_ref().cfg.chain_id
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
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        let tx = CustomEvmTransaction {
            base: CustomTxEnv(TxEnv {
                caller,
                kind: TxKind::Call(contract),
                // Explicitly set nonce to 0 so revm does not do any nonce checks
                nonce: 0,
                gas_limit: 30_000_000,
                value: U256::ZERO,
                data,
                // Setting the gas price to zero enforces that no value is transferred as part of
                // the call, and that the call will not count against the block's
                // gas limit
                gas_price: 0,
                // The chain ID check is not relevant here and is disabled if set to None
                chain_id: None,
                // Setting the gas priority fee to None ensures the effective gas price is derived
                // from the `gas_price` field, which we need to be zero
                gas_priority_fee: None,
                access_list: Default::default(),
                // blob fields can be None for this tx
                blob_hashes: Vec::new(),
                max_fee_per_blob_gas: 0,
                tx_type: OpTxType::Deposit as u8,
                authorization_list: Default::default(),
            }),
            // The L1 fee is not charged for the EIP-4788 transaction, submit zero bytes for the
            // enveloped tx size.
            enveloped_tx: Some(Bytes::default()),
            deposit: Default::default(),
        };

        let mut gas_limit = tx.base.0.gas_limit;
        let mut basefee = 0;
        let mut disable_nonce_check = true;

        // ensure the block gas limit is >= the tx
        core::mem::swap(&mut self.inner.ctx().block.gas_limit, &mut gas_limit);
        // disable the base fee check for this call by setting the base fee to zero
        core::mem::swap(&mut self.inner.ctx().block.basefee, &mut basefee);
        // disable the nonce check
        core::mem::swap(&mut self.inner.ctx().cfg.disable_nonce_check, &mut disable_nonce_check);

        let mut res = self.transact(tx);

        // swap back to the previous gas limit
        core::mem::swap(&mut self.inner.ctx().block.gas_limit, &mut gas_limit);
        // swap back to the previous base fee
        core::mem::swap(&mut self.inner.ctx().block.basefee, &mut basefee);
        // swap back to the previous nonce check flag
        core::mem::swap(&mut self.inner.ctx().cfg.disable_nonce_check, &mut disable_nonce_check);

        // NOTE: We assume that only the contract storage is modified. Revm currently marks the
        // caller and block beneficiary accounts as "touched" when we do the above transact calls,
        // and includes them in the result.
        //
        // We're doing this state cleanup to make sure that changeset only includes the changed
        // contract storage.
        if let Ok(res) = &mut res {
            res.state.retain(|addr, _| *addr == contract);
        }

        res
    }

    fn db_mut(&mut self) -> &mut Self::DB {
        &mut self.inner.ctx().journaled_state.database
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec>) {
        let Context { block: block_env, cfg: cfg_env, journaled_state, .. } = self.inner.0.ctx;

        (journaled_state.database, EvmEnv { block_env, cfg_env })
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.inspect = enabled;
    }

    fn precompiles(&self) -> &Self::Precompiles {
        &self.inner.0.precompiles
    }

    fn precompiles_mut(&mut self) -> &mut Self::Precompiles {
        &mut self.inner.0.precompiles
    }

    fn inspector(&self) -> &Self::Inspector {
        &self.inner.0.inspector
    }

    fn inspector_mut(&mut self) -> &mut Self::Inspector {
        &mut self.inner.0.inspector
    }
}

pub struct CustomEvmFactory;

impl EvmFactory for CustomEvmFactory {
    type Evm<DB: Database, I: Inspector<CustomContext<DB>>> = CustomEvm<DB, I, Self::Precompiles>;
    type Context<DB: Database> = CustomContext<DB>;
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
            inner: Context::op()
                .with_tx(CustomEvmTransaction {
                    base: CustomTxEnv::default(),
                    enveloped_tx: Some(vec![0x00].into()),
                    deposit: DepositTransactionParts::default(),
                })
                .with_db(db)
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .build_op_with_inspector(NoOpInspector {})
                .with_precompiles(PrecompilesMap::from_static(
                    OpPrecompiles::new_with_spec(spec_id).precompiles(),
                )),
            inspect: false,
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
            inner: Context::op()
                .with_tx(CustomEvmTransaction {
                    base: CustomTxEnv::default(),
                    enveloped_tx: Some(vec![0x00].into()),
                    deposit: DepositTransactionParts::default(),
                })
                .with_db(db)
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .build_op_with_inspector(inspector)
                .with_precompiles(PrecompilesMap::from_static(
                    OpPrecompiles::new_with_spec(spec_id).precompiles(),
                )),
            inspect: true,
        }
    }
}
