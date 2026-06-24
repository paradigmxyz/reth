//! Helper aliases when working with [`ConfigureEvm`] and the traits in this crate.

use crate::ConfigureEvm;

/// Type alias for the configured EVM environment.
pub type EvmEnvFor<Evm> = <Evm as ConfigureEvm>::EvmEnv;

/// Type alias for the configured transaction environment.
pub type TxEnvFor<Evm> = <Evm as ConfigureEvm>::TxEnv;

/// Helper to access the configured execution context.
pub type ExecutionCtxFor<'a, Evm> = <Evm as ConfigureEvm>::ExecutionCtx<'a>;

/// Helper to access the configured block executor.
#[cfg(feature = "std")]
pub type BlockExecutorFor<'a, Evm, DB> =
    <<Evm as ConfigureEvm>::BlockExecutorFactory as crate::execute::BlockExecutorFactory>::Executor<
        'a,
        DB,
    >;
