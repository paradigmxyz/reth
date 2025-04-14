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
#[cfg(feature = "std")]
use mini_moka::sync::CacheBuilder;
#[cfg(feature = "metrics")]
use reth_metrics::Metrics;

#[cfg(feature = "std")]
type Cache<K, V> = mini_moka::sync::Cache<K, V, alloy_primitives::map::DefaultHashBuilder>;

#[cfg(feature = "std")]
type CacheKey = (SpecId, Bytes);

#[cfg(feature = "std")]
/// Type alias for the LRU cache used within the [`PrecompileCache`].
type PrecompileLRUCache = Cache<CacheKey, Result<InterpreterResult, String>>;

/// Gas interval outside of which we can we know how the precompile behaves in
/// terms of gas.
///
/// Our precompile cache contains the results of the precompile calls, and those
/// depend both on the input and the gas limit. If we include the gas limit in
/// the cache key the hit rate will be very low.
/// Together with the precompile address we store in this struct the gas limit
/// values that determine when the precompile fails because of out of gas. We
/// don't know exactly what is the gas cost of each precompile, and store these
/// values according to the results we observe.
/// This will help to optimize the cache perfomance:
///   * if a request comes with enough gas we know it won't fail with out of gas and can store it in
///     the cache without the concrete gas limit
///   * if a request comes with not enough gas we know it will fail with out of gas
#[derive(Debug)]
#[cfg(feature = "std")]
struct PrecompileGasBounds {
    /// Observed gas limit above which the precompile does not fail with out of gas.
    upper_gas_limit: u64,
    /// Observed gas limit below which the precompile fails with out of gas.
    lower_gas_limit: u64,
}

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

    /// Precompile gas bounds.
    #[cfg(feature = "std")]
    precompile_gas_bounds: DashMap<Address, PrecompileGasBounds>,
}

/// Metrics for the cached precompile provider, showing hits / misses for each cache
#[cfg(feature = "metrics")]
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
            // First, check if we have precompile info and can make a quick decision
            if let Some(info) = cache.precompile_gas_bounds.get(address) {
                // If gas_limit is below known lower bound, we know it will fail with OOG
                if gas_limit <= info.lower_gas_limit {
                    return Err("out of gas".to_string());
                }

                // We have enough information to use the cache without the gas limit
                if gas_limit >= info.upper_gas_limit {
                    // Create a key without the gas limit
                    let key = (self.spec, inputs.input.clone());

                    // Check if we have a cached result
                    if let Some(precompiles) = cache.cache.get(address) {
                        #[cfg(feature = "metrics")]
                        self.metrics.precompile_cache_hits.increment(1);

                        if let Some(result) = precompiles.get(&key) {
                            return result.map(Some);
                        }
                    }
                }
            }

            #[cfg(feature = "metrics")]
            self.metrics.precompile_cache_misses.increment(1);

            // call the precompile if cache miss
            let output =
                self.precompile_provider.run(context, address, inputs, is_static, gas_limit);

            match &output {
                Ok(Some(result)) => {
                    // Create or update the precompile info
                    cache
                        .precompile_gas_bounds
                        .entry(*address)
                        .and_modify(|info| {
                            // Update the upper gas limit if this was successful
                            info.upper_gas_limit = info.upper_gas_limit.min(gas_limit);
                        })
                        .or_insert(PrecompileGasBounds {
                            upper_gas_limit: gas_limit,
                            lower_gas_limit: 0, // We don't know the lower bound yet
                        });

                    // Cache the successful result without the gas limit
                    let key = (self.spec, inputs.input.clone());

                    cache_result(
                        cache,
                        address,
                        key,
                        Ok(result.clone()),
                        #[cfg(feature = "metrics")]
                        &self.metrics,
                    );
                }
                Err(err) => {
                    // If error is "out of gas", update the lower gas limit
                    if err.contains("out of gas") {
                        // update the gas bounds for out of gas errors
                        cache
                            .precompile_gas_bounds
                            .entry(*address)
                            .and_modify(|info| {
                                // Update the lower gas limit if this was an OOG failure
                                info.lower_gas_limit = info.lower_gas_limit.max(gas_limit);
                            })
                            .or_insert(PrecompileGasBounds {
                                upper_gas_limit: u64::MAX, // We don't know the upper bound yet
                                lower_gas_limit: gas_limit,
                            });
                    } else {
                        // for non-OOG errors, still cache the error
                        let key = (self.spec, inputs.input.clone());
                        cache_result(
                            cache,
                            address,
                            key,
                            Err(err.clone()),
                            #[cfg(feature = "metrics")]
                            &self.metrics,
                        );
                    }
                }
                Ok(None) => {
                    // precompile chose not to handle this request
                    // no caching needed
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

#[cfg(feature = "std")]
fn cache_result(
    cache: &PrecompileCache,
    address: &Address,
    key: CacheKey,
    result: Result<InterpreterResult, String>,
    #[cfg(feature = "metrics")] metrics: &CachedPrecompileMetrics,
) {
    // Create or get the cache for this address
    #[cfg(feature = "metrics")]
    let is_new_address = !cache.cache.contains_key(address);
    #[cfg(feature = "metrics")]
    let entry_count_before = if is_new_address {
        0
    } else if let Some(precompile_cache) = cache.cache.get(address) {
        precompile_cache.entry_count()
    } else {
        0
    };

    cache
        .cache
        .entry(*address)
        .or_insert(CacheBuilder::new(10000).build_with_hasher(DefaultHashBuilder::default()))
        .insert(key, result);

    #[cfg(feature = "metrics")]
    if let Some(precompile_cache) = cache.cache.get(address) {
        let new_entry_count = precompile_cache.entry_count();
        if new_entry_count > entry_count_before {
            metrics.precompile_cache_size.increment((new_entry_count - entry_count_before) as f64);
        }
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

    fn test_input(data: &[u8]) -> (SpecId, Bytes) {
        (SpecId::PRAGUE, Bytes::copy_from_slice(data))
    }

    #[test]
    fn test_precompile_cache_basic() {
        let cache = Arc::new(PrecompileCache::default());

        let address = precompile_address(1);
        let key = test_input(b"test_input");

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
