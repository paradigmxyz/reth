//! Alloy Evm API for Scroll.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

mod block;
pub use block::{
    curie, feynman, EvmExt, ReceiptBuilderCtx, ScrollBlockExecutionCtx, ScrollBlockExecutor,
    ScrollBlockExecutorFactory, ScrollReceiptBuilder, ScrollTxCompressionRatios,
};

mod tx;
pub use tx::{
    compute_compression_ratio, FromTxWithCompressionRatio, ScrollTransactionIntoTxEnv,
    ToTxWithCompressionRatio, WithCompressionRatio,
};

mod system_caller;

extern crate alloc;

use alloc::vec::Vec;
use alloy_evm::{precompiles::PrecompilesMap, Database, Evm, EvmEnv, EvmFactory};
use alloy_primitives::{Address, Bytes, TxKind, U256};
use core::{
    fmt,
    ops::{Deref, DerefMut},
};
use revm::{
    context::{result::HaltReason, BlockEnv, TxEnv},
    context_interface::result::{EVMError, ResultAndState},
    handler::PrecompileProvider,
    inspector::NoOpInspector,
    interpreter::{interpreter::EthInterpreter, InterpreterResult},
    Context, ExecuteEvm, InspectEvm, Inspector,
};
use revm_scroll::{
    builder::{
        DefaultScrollContext, EuclidEipActivations, FeynmanEipActivations, ScrollBuilder,
        ScrollContext,
    },
    instructions::ScrollInstructions,
    precompile::ScrollPrecompileProvider,
    ScrollSpecId, ScrollTransaction,
};

/// Re-export `TX_L1_FEE_PRECISION_U256` from `revm-scroll` for convenience.
pub use revm_scroll::l1block::TX_L1_FEE_PRECISION_U256;

/// Scroll EVM implementation.
#[allow(missing_debug_implementations)]
pub struct ScrollEvm<DB: Database, I, P = ScrollPrecompileProvider> {
    inner: revm_scroll::ScrollEvm<
        ScrollContext<DB>,
        I,
        ScrollInstructions<EthInterpreter, ScrollContext<DB>>,
        P,
    >,
    inspect: bool,
}

impl<DB: Database, I, P> ScrollEvm<DB, I, P> {
    /// Creates a new instance of [`ScrollEvm`].
    pub const fn new(
        inner: revm_scroll::ScrollEvm<
            ScrollContext<DB>,
            I,
            ScrollInstructions<EthInterpreter, ScrollContext<DB>>,
            P,
        >,
        inspect: bool,
    ) -> Self {
        Self { inner, inspect }
    }

    /// Provides a reference to the EVM context.
    pub const fn ctx(&self) -> &ScrollContext<DB> {
        &self.inner.0.ctx
    }

    /// Provides a mutable reference to the EVM context.
    pub const fn ctx_mut(&mut self) -> &mut ScrollContext<DB> {
        &mut self.inner.0.ctx
    }
}

impl<DB: Database, I, P> Deref for ScrollEvm<DB, I, P> {
    type Target = ScrollContext<DB>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.ctx()
    }
}

impl<DB: Database, I, P> DerefMut for ScrollEvm<DB, I, P> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ctx_mut()
    }
}

impl<DB, I, P> Evm for ScrollEvm<DB, I, P>
where
    DB: Database,
    I: Inspector<ScrollContext<DB>>,
    P: PrecompileProvider<ScrollContext<DB>, Output = InterpreterResult>,
{
    type DB = DB;
    type Tx = ScrollTransactionIntoTxEnv<TxEnv>;
    type Error = EVMError<DB::Error>;
    type HaltReason = HaltReason;
    type Spec = ScrollSpecId;
    type Precompiles = P;
    type Inspector = I;

    fn block(&self) -> &BlockEnv {
        &self.block
    }

    fn chain_id(&self) -> u64 {
        self.cfg.chain_id
    }

    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        if self.inspect {
            self.inner.set_tx(tx.into());
            self.inner.inspect_replay()
        } else {
            self.inner.transact(tx.into())
        }
    }

    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        let tx = ScrollTransaction {
            base: TxEnv {
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
                tx_type: 0,
                authorization_list: Default::default(),
            },
            rlp_bytes: Some(Default::default()),
            // System transactions (similar to L1MessageTx) do not pay a rollup fee,
            // so this field is not used; we just set it to the default value.
            compression_ratio: Some(TX_L1_FEE_PRECISION_U256),
        };

        let mut gas_limit = tx.base.gas_limit;
        let mut basefee = 0;
        let mut disable_nonce_check = true;

        // ensure the block gas limit is >= the tx
        core::mem::swap(&mut self.block.gas_limit, &mut gas_limit);
        // disable the base fee check for this call by setting the base fee to zero
        core::mem::swap(&mut self.block.basefee, &mut basefee);
        // disable the nonce check
        core::mem::swap(&mut self.cfg.disable_nonce_check, &mut disable_nonce_check);

        let res = self.transact(ScrollTransactionIntoTxEnv::from(tx));

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

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec>)
    where
        Self: Sized,
    {
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

/// Factory producing [`ScrollEvm`]s.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct ScrollEvmFactory<P = ScrollDefaultPrecompilesFactory> {
    _precompiles_factory: core::marker::PhantomData<P>,
}

impl<P: ScrollPrecompilesFactory> EvmFactory for ScrollEvmFactory<P> {
    type Evm<DB: Database, I: Inspector<ScrollContext<DB>>> = ScrollEvm<DB, I, Self::Precompiles>;
    type Context<DB: Database> = ScrollContext<DB>;
    type Tx = ScrollTransactionIntoTxEnv<TxEnv>;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Spec = ScrollSpecId;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv<ScrollSpecId>,
    ) -> Self::Evm<DB, NoOpInspector> {
        let spec_id = input.cfg_env.spec;
        ScrollEvm {
            inner: Context::scroll()
                .with_db(db)
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .maybe_with_eip_7702()
                .maybe_with_eip_7623()
                .build_scroll_with_inspector(NoOpInspector {})
                .with_precompiles(P::with_spec(spec_id)),
            inspect: false,
        }
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<ScrollSpecId>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        let spec_id = input.cfg_env.spec;
        ScrollEvm {
            inner: Context::scroll()
                .with_db(db)
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .maybe_with_eip_7702()
                .maybe_with_eip_7623()
                .build_scroll_with_inspector(inspector)
                .with_precompiles(P::with_spec(spec_id)),
            inspect: true,
        }
    }
}

/// A factory trait for creating precompiles for Scroll EVM.
pub trait ScrollPrecompilesFactory: Default + fmt::Debug {
    /// Creates a new instance of precompiles for the given Scroll specification ID.
    fn with_spec(spec: ScrollSpecId) -> PrecompilesMap;
}

/// Default implementation of the Scroll precompiles factory.
#[derive(Default, Debug, Copy, Clone)]
pub struct ScrollDefaultPrecompilesFactory;

impl ScrollPrecompilesFactory for ScrollDefaultPrecompilesFactory {
    fn with_spec(spec_id: ScrollSpecId) -> PrecompilesMap {
        PrecompilesMap::from_static(ScrollPrecompileProvider::new_with_spec(spec_id).precompiles())
    }
}
