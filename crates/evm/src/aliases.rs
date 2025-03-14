//! Helper aliases when working with [`ConfigureEvm`] and the traits in this crate.

use crate::ConfigureEvm;
use alloy_evm::{block::BlockExecutorFactory, Database, EvmEnv, EvmFactory};
use revm::{inspector::NoOpInspector, Inspector};

/// Helper to access [`EvmFactory`] for a given [`ConfigureEvm`].
pub type EvmFactoryFor<Evm> =
    <<Evm as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::EvmFactory;

/// Helper to access [`EvmFactory::Spec`] for a given [`ConfigureEvm`].
pub type SpecFor<Evm> = <EvmFactoryFor<Evm> as EvmFactory>::Spec;

/// Helper to access [`EvmFactory::Evm`] for a given [`ConfigureEvm`].
pub type EvmFor<Evm, DB, I = NoOpInspector> = <EvmFactoryFor<Evm> as EvmFactory>::Evm<DB, I>;

/// Helper to access [`EvmFactory::Error`] for a given [`ConfigureEvm`].
pub type EvmErrorFor<Evm, DB> = <EvmFactoryFor<Evm> as EvmFactory>::Error<DB>;

/// Helper to access [`EvmFactory::Context`] for a given [`ConfigureEvm`].
pub type EvmContextFor<Evm, DB> = <EvmFactoryFor<Evm> as EvmFactory>::Context<DB>;

/// Helper to access [`EvmFactory::HaltReason`] for a given [`ConfigureEvm`].
pub type HaltReasonFor<Evm> = <EvmFactoryFor<Evm> as EvmFactory>::HaltReason;

/// Helper to access [`EvmFactory::Tx`] for a given [`ConfigureEvm`].
pub type TxEnvFor<Evm> = <EvmFactoryFor<Evm> as EvmFactory>::Tx;

/// Helper to access [`BlockExecutorFactory::ExecutionCtx`] for a given [`ConfigureEvm`].
pub type ExecutionCtxFor<'a, Evm> =
    <<Evm as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::ExecutionCtx<'a>;

/// Type alias for [`EvmEnv`] for a given [`ConfigureEvm`].
pub type EvmEnvFor<Evm> = EvmEnv<SpecFor<Evm>>;

/// Helper trait to bound [`Inspector`] for a [`ConfigureEvm`].
pub trait InspectorFor<Evm: ConfigureEvm, DB: Database>: Inspector<EvmContextFor<Evm, DB>> {}
impl<T, Evm, DB> InspectorFor<Evm, DB> for T
where
    Evm: ConfigureEvm,
    DB: Database,
    T: Inspector<EvmContextFor<Evm, DB>>,
{
}
