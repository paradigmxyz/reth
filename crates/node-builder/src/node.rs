use reth_node_api::{
    node::{FullNodeTypes, NodeTypes},
    provider::FullProvider,
};
use std::marker::PhantomData;

/// An adapter type that adds the builtin provider type to the user configured node types.
#[derive(Debug)]
pub struct FullNodeTypesAdapter<Types, Provider> {
    _types: PhantomData<Types>,
    _provider: PhantomData<Provider>,
}

impl<Types, Provider> Default for FullNodeTypesAdapter<Types, Provider> {
    fn default() -> Self {
        Self { _types: Default::default(), _provider: Default::default() }
    }
}

impl<Types, Provider> NodeTypes for FullNodeTypesAdapter<Types, Provider>
where
    Types: NodeTypes,
    Provider: Send + Sync + 'static,
{
    type Primitives = Types::Primitives;
    type Engine = Types::Engine;
    type Evm = Types::Evm;
}

impl<Types, Provider> FullNodeTypes for FullNodeTypesAdapter<Types, Provider>
where
    Types: NodeTypes,
    Provider: FullProvider,
{
    type Provider = Provider;
}
