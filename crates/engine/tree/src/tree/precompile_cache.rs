//! Contains a precompile cache that is backed by a moka cache.

use crate::tree::cached_state::Cache;
use alloy_evm::{eth::EthEvmContext, EvmFactory};
use alloy_primitives::{map::DefaultHashBuilder, Address, Bytes};
use dashmap::DashMap;
use mini_moka::sync::CacheBuilder;
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::{Database, EvmEnv};
use reth_revm::revm::{
    context::{Cfg, Context, TxEnv},
    context_interface::{
        result::{EVMError, HaltReason},
        ContextTr,
    },
    handler::{EthPrecompiles, PrecompileProvider},
    inspector::{Inspector, NoOpInspector},
    interpreter::{interpreter::EthInterpreter, InterpreterResult, InputsImpl},
    precompile::PrecompileError,
    primitives::hardfork::SpecId,
    MainBuilder, MainContext,
};
use std::{collections::HashMap, sync::Arc};

/// Type alias for the LRU cache used within the [`PrecompileCache`].
type PrecompileLRUCache = Cache<(SpecId, Bytes, u64), Result<InterpreterResult, String>>;

/// A cache for precompile inputs / outputs.
///
/// This assumes that the precompile is a standard precompile, as in `StandardPrecompileFn`, meaning
/// its inputs are only `(Bytes, u64)`.
///
/// NOTE: This does not work with "context stateful precompiles", ie `ContextStatefulPrecompile` or
/// `ContextStatefulPrecompileMut`. They are explicitly banned.
#[derive(Debug, Default)]
pub struct PrecompileCache {
    /// Caches for each precompile input / output.
    cache: DashMap<Address, PrecompileLRUCache>,
}

/// A custom precompile that contains the cache and precompile it wraps.
#[derive(Clone)]
pub struct WrappedPrecompile<P> {
    /// The precompile to wrap.
    precompile: P,
    /// The cache to use.
    cache: Arc<PrecompileCache>,
    /// The spec id to use.
    spec: SpecId,
}

impl<P> WrappedPrecompile<P> {
    /// Given a [`PrecompileProvider`] and cache for a specific precompiles, create a
    /// wrapper that can be used inside Evm.
    fn new(precompile: P, cache: Arc<PrecompileCache>) -> Self {
        WrappedPrecompile { precompile, cache: cache.clone(), spec: SpecId::default() }
    }
}

impl<CTX: ContextTr, P: PrecompileProvider<CTX, Output = InterpreterResult>> PrecompileProvider<CTX>
    for WrappedPrecompile<P>
{
    type Output = P::Output;

    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) -> bool {
        self.precompile.set_spec(spec.clone());
        self.spec = spec.into();
        true
    }

    fn run(
        &mut self,
        context: &mut CTX,
        address: &Address,
        inputs: &InputsImpl,
        is_static: bool,
        gas_limit: u64,
    ) -> Result<Option<Self::Output>, String> {
        let key = (self.spec, inputs.input.clone(), gas_limit);

        // get the result if it exists
        if let Some(precompiles) = self.cache.cache.get_mut(address) {
            if let Some(result) = precompiles.get(&key) {
                return result.clone().map(Some)
            }
        }

        // call the precompile if cache miss
        let output = self.precompile.run(context, address, inputs, is_static, gas_limit);

        if let Some(output) = output.clone().transpose() {
            // insert the result into the cache
            self
                .cache
                .cache
                .entry(*address)
                // TODO: use a better cache size
                .or_insert(CacheBuilder::new(10000).build_with_hasher(DefaultHashBuilder::default()))
                .insert(key, output);
        }

        output
    }

    fn warm_addresses(&self) -> Box<impl Iterator<Item = Address>> {
        self.precompile.warm_addresses()
    }

    fn contains(&self, address: &Address) -> bool {
        self.precompile.contains(address)
    }
}
