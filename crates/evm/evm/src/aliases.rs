//! Helper aliases when working with [`ConfigureEvm`] and the traits in this crate.

use crate::ConfigureEvm;

/// Helper to access the evm2 spec for a given [`ConfigureEvm`].
pub type SpecFor<Evm> = <Evm as ConfigureEvm>::Spec;

/// Type alias for the configured EVM environment.
pub type EvmEnvFor<Evm> = <Evm as ConfigureEvm>::EvmEnv;

/// Type alias for the configured transaction environment.
pub type TxEnvFor<Evm> = <Evm as ConfigureEvm>::TxEnv;

/// Helper to access the configured execution context.
pub type ExecutionCtxFor<'a, Evm> = <Evm as ConfigureEvm>::ExecutionCtx<'a>;
