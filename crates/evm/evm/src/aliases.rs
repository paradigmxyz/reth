//! Helper aliases when working with [`ConfigureEvm`] and the traits in this crate.

use crate::{execute::BlockExecutorFactory, ConfigureEvm};

/// Helper to access the configured EVM factory state.
pub type EvmFactoryFor<Evm> =
    <<Evm as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::EvmFactory;

/// Type alias for the configured EVM environment.
pub type EvmEnvFor<Evm> =
    <<Evm as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::EvmEnv;

/// Type alias for the configured EVM instance.
#[cfg(feature = "std")]
pub type EvmFor<'a, Evm> =
    <<Evm as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::Evm<'a>;

/// Type alias for the configured transaction environment.
pub type TxEnvFor<Evm> =
    <<Evm as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::Transaction;

/// Type alias for the transaction type consumed by the configured EVM instance.
pub type EvmTransactionFor<Evm> =
    <<Evm as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::EvmTransaction;

/// Helper to access the configured execution context.
pub type ExecutionCtxFor<'a, Evm> =
    <<Evm as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::ExecutionCtx<'a>;

/// Helper to access the configured block executor.
#[cfg(feature = "std")]
pub type BlockExecutorFor<'a, Evm> =
    <<Evm as ConfigureEvm>::BlockExecutorFactory as crate::execute::BlockExecutorFactory>::Executor<
        'a,
    >;
