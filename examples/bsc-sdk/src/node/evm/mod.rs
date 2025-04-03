use crate::{
    chainspec::BscChainSpec,
    evm::{
        api::{ctx::BscContext, BscEvmInner},
        precompiles::BscPrecompiles,
        spec::BscSpecId,
        transaction::BscTransaction,
    },
};
use alloy_primitives::{Address, Bytes, TxKind, U256};
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

    /// Provides a mutable reference to the EVM inspector.
    pub fn inspector_mut(&mut self) -> &mut I {
        &mut self.inner.0.data.inspector
    }
}

impl<DB: Database, I, P> BscEvm<DB, I, P> {
    /// Creates a new Bsc EVM instance.
    ///
    /// The `inspect` argument determines whether the configured [`Inspector`] of the given
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
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        // TODO: error handling
        let account = self.db_mut().basic(caller).unwrap().unwrap_or_default();

        println!("account: {:?}", account.nonce);

        let tx = TxEnv {
            caller,
            kind: TxKind::Call(contract),
            nonce: account.nonce,
            gas_limit: u64::MAX / 2,
            value: U256::ZERO,
            data,
            // Setting the gas price to zero enforces that no value is transferred as part of the
            // call, and that the call will not count against the block's gas limit
            gas_price: 0,
            // The chain ID check is not relevant here and is disabled if set to None
            chain_id: Some(56),
            // Setting the gas priority fee to None ensures the effective gas price is derived from
            // the `gas_price` field, which we need to be zero
            gas_priority_fee: None,
            access_list: Default::default(),
            // blob fields can be None for this tx
            blob_hashes: Vec::new(),
            max_fee_per_blob_gas: 0,
            tx_type: 0,
            authorization_list: Default::default(),
        };

        let mut gas_limit = tx.gas_limit;
        let mut basefee = 0;
        let mut disable_nonce_check = true;

        // ensure the block gas limit is >= the tx
        core::mem::swap(&mut self.block.gas_limit, &mut gas_limit);
        // disable the base fee check for this call by setting the base fee to zero
        core::mem::swap(&mut self.block.basefee, &mut basefee);
        // disable the nonce check
        core::mem::swap(&mut self.cfg.disable_nonce_check, &mut disable_nonce_check);

        let tx = BscTransaction { base: tx, is_system_transaction: Some(true) };

        let res = self.transact(tx);

        // swap back to the previous gas limit
        core::mem::swap(&mut self.block.gas_limit, &mut gas_limit);
        // swap back to the previous base fee
        core::mem::swap(&mut self.block.basefee, &mut basefee);
        // swap back to the previous nonce check flag
        core::mem::swap(&mut self.cfg.disable_nonce_check, &mut disable_nonce_check);

        res
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
