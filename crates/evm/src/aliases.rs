//! Helper aliases when working with [`NodePrimitives`] and the traits in this crate.
use crate::{ConfigureEvm, ConfigureEvmEnv};
use reth_primitives_traits::NodePrimitives;

/// This is a type alias to make type bounds simpler when we have a [`NodePrimitives`] and need a
/// [`ConfigureEvmEnv`] whose associated types match the [`NodePrimitives`] associated types.
pub trait ConfigureEvmEnvFor<N: NodePrimitives>:
    ConfigureEvmEnv<Header = N::BlockHeader, Transaction = N::SignedTx>
{
}

impl<N, C> ConfigureEvmEnvFor<N> for C
where
    N: NodePrimitives,
    C: ConfigureEvmEnv<Header = N::BlockHeader, Transaction = N::SignedTx>,
{
}

/// This is a type alias to make type bounds simpler when we have a [`NodePrimitives`] and need a
/// [`ConfigureEvm`] whose associated types match the [`NodePrimitives`] associated types.
pub trait ConfigureEvmFor<N: NodePrimitives>:
    ConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>
{
}

impl<N, C> ConfigureEvmFor<N> for C
where
    N: NodePrimitives,
    C: ConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>,
{
}
