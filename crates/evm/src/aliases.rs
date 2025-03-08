//! Helper aliases when working with [`ConfigureEvmEnv`] and the traits in this crate.

use crate::ConfigureEvmEnv;
use alloy_evm::{block::BlockExecutorFactory, Database, EvmEnv, EvmFactory};
use revm::{inspector::NoOpInspector, Inspector};

/// Helper to access [`EvmFactory`] for a given [`ConfigureEvmEnv`].
pub type EvmFactoryFor<Evm> =
    <<Evm as ConfigureEvmEnv>::BlockExecutorFactory as BlockExecutorFactory>::EvmFactory;

/// Helper to access [`EvmFactory::Spec`] for a given [`ConfigureEvmEnv`].
pub type SpecFor<Evm> = <EvmFactoryFor<Evm> as EvmFactory>::Spec;

/// Helper to access [`EvmFactory::Evm`] for a given [`ConfigureEvmEnv`].
pub type EvmFor<Evm, DB, I = NoOpInspector> = <EvmFactoryFor<Evm> as EvmFactory>::Evm<DB, I>;

/// Helper to access [`EvmFactory::Error`] for a given [`ConfigureEvmEnv`].
pub type EvmErrorFor<Evm, DB> = <EvmFactoryFor<Evm> as EvmFactory>::Error<DB>;

/// Helper to access [`EvmFactory::Context`] for a given [`ConfigureEvmEnv`].
pub type EvmContextFor<Evm, DB> = <EvmFactoryFor<Evm> as EvmFactory>::Context<DB>;

/// Helper to access [`EvmFactory::HaltReason`] for a given [`ConfigureEvmEnv`].
pub type HaltReasonFor<Evm> = <EvmFactoryFor<Evm> as EvmFactory>::HaltReason;

/// Helper to access [`EvmFactory::Tx`] for a given [`ConfigureEvmEnv`].
pub type TxEnvFor<Evm> = <EvmFactoryFor<Evm> as EvmFactory>::Tx;

/// Helper to access [`BlockExecutorFactory::ExecutionCtx`] for a given [`ConfigureEvmEnv`].
pub type ExecutionCtxFor<'a, Evm> =
    <<Evm as ConfigureEvmEnv>::BlockExecutorFactory as BlockExecutorFactory>::ExecutionCtx<'a>;

/// Type alias for [`EvmEnv`] for a given [`ConfigureEvmEnv`].
pub type EvmEnvFor<Evm> = EvmEnv<SpecFor<Evm>>;

/// Helper trait to bound [`Inspector`] for a [`ConfigureEvmEnv`].
pub trait InspectorFor<Evm: ConfigureEvmEnv, DB: Database>:
    Inspector<EvmContextFor<Evm, DB>>
{
}
impl<T, Evm, DB> InspectorFor<Evm, DB> for T
where
    Evm: ConfigureEvmEnv,
    DB: Database,
    T: Inspector<EvmContextFor<Evm, DB>>,
{
}
