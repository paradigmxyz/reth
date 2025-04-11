//! Contains a precompile cache that is backed by a moka cache.

use alloc::{boxed::Box, string::String, sync::Arc};
use alloy_primitives::Address;
use reth_revm::revm::{
    context::Cfg,
    context_interface::ContextTr,
    handler::PrecompileProvider,
    interpreter::{InputsImpl, InterpreterResult},
    primitives::hardfork::SpecId,
};

#[cfg(feature = "std")]
use alloy_primitives::{map::DefaultHashBuilder, Bytes};
#[cfg(feature = "std")]
use dashmap::DashMap;
#[cfg(feature = "metrics")]
use metrics::Gauge;
#[cfg(feature = "metrics")]
use mini_moka::sync::CacheBuilder;
#[cfg(feature = "std")]
use reth_metrics::Metrics;

#[cfg(feature = "std")]
type Cache<K, V> = mini_moka::sync::Cache<K, V, alloy_primitives::map::DefaultHashBuilder>;

#[cfg(feature = "std")]
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
    #[cfg(feature = "std")]
    cache: DashMap<Address, PrecompileLRUCache>,
}

/// Metrics for the cached precompile provider, showing hits / misses for each cache
#[cfg(feature = "std")]
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.caching")]
pub(crate) struct CachedPrecompileMetrics {
    /// Precompile cache hits
    precompile_cache_hits: Gauge,

    /// Precompile cache misses
    precompile_cache_misses: Gauge,

    /// Precompile cache size
    ///
    /// NOTE: this uses the moka caches' `entry_count`, NOT the `weighted_size` method to calculate
    /// size.
    precompile_cache_size: Gauge,
}

/// A custom precompile provider that wraps a precompile provider and potentially a cache for it.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct MaybeCachedPrecompileProvider<P> {
    /// The precompile provider to wrap.
    precompile_provider: P,
    /// The cache to use.
    cache: Option<Arc<PrecompileCache>>,
    /// The spec id to use.
    spec: SpecId,
    /// Cache metrics.
    #[cfg(feature = "metrics")]
    metrics: CachedPrecompileMetrics,
}

impl<P> MaybeCachedPrecompileProvider<P> {
    /// Given a [`PrecompileProvider`]  and cache for a specific precompile provider,
    /// create a cached wrapper that can be used inside Evm.
    #[cfg(feature = "std")]
    pub fn new_with_cache(precompile_provider: P, cache: Arc<PrecompileCache>) -> Self {
        Self {
            precompile_provider,
            cache: Some(cache),
            spec: SpecId::default(),
            #[cfg(feature = "metrics")]
            metrics: Default::default(),
        }
    }

    /// Creates a new `MaybeCachedPrecompileProvider` with cache disabled.
    pub fn new_without_cache(precompile_provider: P) -> Self {
        Self {
            precompile_provider,
            cache: None,
            spec: Default::default(),
            #[cfg(feature = "metrics")]
            metrics: Default::default(),
        }
    }

    #[cfg(not(feature = "std"))]
    /// Creates a new `MaybeCachedPrecompileProvider` with cache disabled in no-std environments.
    pub fn new_with_cache(precompile_provider: P, _cache: Arc<PrecompileCache>) -> Self {
        // In no-std environments, always return no-cache version
        Self::new_without_cache(precompile_provider)
    }
}

impl<CTX: ContextTr, P: PrecompileProvider<CTX, Output = InterpreterResult>> PrecompileProvider<CTX>
    for MaybeCachedPrecompileProvider<P>
{
    type Output = P::Output;

    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) -> bool {
        self.precompile_provider.set_spec(spec.clone());
        self.spec = spec.into();
        true
    }

    #[cfg(feature = "std")]
    fn run(
        &mut self,
        context: &mut CTX,
        address: &Address,
        inputs: &InputsImpl,
        is_static: bool,
        gas_limit: u64,
    ) -> Result<Option<Self::Output>, String> {
        if let Some(cache) = &self.cache {
            let key = (self.spec, inputs.input.clone(), gas_limit);

            // get the result if it exists
            if let Some(precompiles) = cache.cache.get_mut(address) {
                #[cfg(feature = "metrics")]
                self.metrics.precompile_cache_hits.increment(1);
                if let Some(result) = precompiles.get(&key) {
                    return result.map(Some)
                }
            }
            #[cfg(feature = "metrics")]
            self.metrics.precompile_cache_misses.increment(1);

            // call the precompile if cache miss
            let output =
                self.precompile_provider.run(context, address, inputs, is_static, gas_limit);

            if let Some(output) = output.clone().transpose() {
                // Check if entry exists for this address before insertion
                let is_new_address = !cache.cache.contains_key(address);
                let entry_count_before = if is_new_address {
                    0
                } else if let Some(precompile_cache) = cache.cache.get(address) {
                    precompile_cache.entry_count()
                } else {
                    0
                };

                // insert the result into the cache
                cache
                    .cache
                    .entry(*address)
                    // TODO: use a better cache size
                    .or_insert(
                        CacheBuilder::new(10000).build_with_hasher(DefaultHashBuilder::default()),
                    )
                    .insert(key, output);

                #[cfg(feature = "metrics")]
                if let Some(precompile_cache) = cache.cache.get(address) {
                    let new_entry_count = precompile_cache.entry_count();
                    if new_entry_count > entry_count_before {
                        // Increment the cache size metric by the number of new entries
                        self.metrics
                            .precompile_cache_size
                            .increment((new_entry_count - entry_count_before) as f64);
                    }
                }
            }

            output
        } else {
            self.precompile_provider.run(context, address, inputs, is_static, gas_limit)
        }
    }

    #[cfg(not(feature = "std"))]
    fn run(
        &mut self,
        context: &mut CTX,
        address: &Address,
        inputs: &InputsImpl,
        is_static: bool,
        gas_limit: u64,
    ) -> Result<Option<Self::Output>, String> {
        // In no-std environments, always directly run the precompile without caching
        self.precompile_provider.run(context, address, inputs, is_static, gas_limit)
    }

    fn warm_addresses(&self) -> Box<impl Iterator<Item = Address>> {
        self.precompile_provider.warm_addresses()
    }

    fn contains(&self, address: &Address) -> bool {
        self.precompile_provider.contains(address)
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use alloy_primitives::map::DefaultHashBuilder;
    use mini_moka::sync::CacheBuilder;
    use reth_revm::revm::interpreter::{Gas, InstructionResult};

    fn precompile_address(num: u8) -> Address {
        Address::from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, num])
    }

    fn test_input(data: &[u8], gas: u64) -> (SpecId, Bytes, u64) {
        (SpecId::PRAGUE, Bytes::copy_from_slice(data), gas)
    }

    #[test]
    fn test_precompile_cache_basic() {
        let cache = Arc::new(PrecompileCache::default());

        let address = precompile_address(1);
        let key = test_input(b"test_input", 100);

        let expected = Ok(InterpreterResult::new(
            InstructionResult::Return,
            Bytes::copy_from_slice(b"cached_result"),
            Gas::new(50),
        ));

        let subcache = CacheBuilder::new(10000).build_with_hasher(DefaultHashBuilder::default());
        cache.cache.insert(address, subcache);
        cache.cache.get_mut(&address).unwrap().insert(key.clone(), expected.clone());

        let actual = cache.cache.get(&address).unwrap().get(&key).unwrap();

        assert_eq!(actual, expected);
    }
}
