//! Contains a precompile cache that is backed by a moka cache.

use alloy_primitives::Address;
use reth_revm::revm::{
    context::Cfg,
    context_interface::ContextTr,
    handler::PrecompileProvider,
    interpreter::{InputsImpl, InterpreterResult},
    primitives::hardfork::SpecId,
};
use std::sync::Arc;

type Cache<K, V> = mini_moka::sync::Cache<K, V, alloy_primitives::map::DefaultHashBuilder>;

/// Cache key, just input for each precompile.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PrecompileKey(alloy_primitives::Bytes);

/// Combined entry containing both the result and gas bounds.
#[derive(Debug, Clone, PartialEq)]
struct CacheEntry {
    /// The actual result of executing the precompile.
    result: Result<InterpreterResult, String>,
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
#[derive(Debug)]
pub struct PrecompileCache {
    /// Cache for precompile results and gas bounds.
    cache: Cache<PrecompileKey, CacheEntry>,
}

impl Default for PrecompileCache {
    fn default() -> Self {
        Self {
            cache: mini_moka::sync::CacheBuilder::new(100_000)
                .build_with_hasher(alloy_primitives::map::DefaultHashBuilder::default()),
        }
    }
}

/// Metrics for the cached precompile provider, showing hits / misses for each cache
#[derive(reth_metrics::Metrics, Clone)]
#[metrics(scope = "sync.caching")]
pub(crate) struct CachedPrecompileMetrics {
    /// Precompile cache hits
    precompile_cache_hits: metrics::Gauge,

    /// Precompile cache misses
    precompile_cache_misses: metrics::Gauge,

    /// Precompile cache size
    ///
    /// NOTE: this uses the moka caches' `entry_count`, NOT the `weighted_size` method to calculate
    /// size.
    precompile_cache_size: metrics::Gauge,
}

/// A custom precompile provider that wraps a precompile provider and potentially a cache for it.
#[derive(Clone, Debug)]
pub struct MaybeCachedPrecompileProvider<P> {
    /// The precompile provider to wrap.
    precompile_provider: P,
    /// The cache to use.
    cache: Option<Arc<PrecompileCache>>,
    /// The spec id to use.
    spec: SpecId,
    /// Cache metrics.
    metrics: CachedPrecompileMetrics,
}

impl<P> MaybeCachedPrecompileProvider<P> {
    /// Given a [`PrecompileProvider`]  and cache for a specific precompile provider,
    /// create a cached wrapper that can be used inside Evm.
    pub fn new_with_cache(precompile_provider: P, cache: Arc<PrecompileCache>) -> Self {
        Self {
            precompile_provider,
            cache: Some(cache),
            spec: Default::default(),

            metrics: Default::default(),
        }
    }

    /// Creates a new `MaybeCachedPrecompileProvider` with cache disabled.
    pub fn new_without_cache(precompile_provider: P) -> Self {
        Self {
            precompile_provider,
            cache: None,
            spec: Default::default(),

            metrics: Default::default(),
        }
    }
}

impl<CTX: ContextTr, P: PrecompileProvider<CTX, Output = InterpreterResult>> PrecompileProvider<CTX>
    for MaybeCachedPrecompileProvider<P>
{
    type Output = P::Output;

    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) -> bool {
        let old_spec = self.spec;
        self.spec = spec.clone().into();

        if self.precompile_provider.set_spec(spec) {
            return true;
        }

        old_spec != self.spec
    }

    fn run(
        &mut self,
        context: &mut CTX,
        address: &Address,
        inputs: &InputsImpl,
        is_static: bool,
        gas_limit: u64,
    ) -> Result<Option<Self::Output>, String> {
        use revm::interpreter::{Gas, InstructionResult};

        // return early if this is not a precompile address
        if !self.precompile_provider.contains(address) {
            return Ok(None);
        }

        if let Some(cache) = &self.cache {
            let key = PrecompileKey(inputs.input.clone());

            let cache_result = cache.cache.get(&key);

            if let Some(ref entry) = cache_result {
                // for each precompile and input we store in lower_gas_limit the maximum gas for
                // which we have received an out of gas error, any gas limit below that will fail
                // with OOG too.
                if gas_limit <= entry.lower_gas_limit {
                    self.increment_by_one_precompile_cache_hits();

                    let result = InterpreterResult {
                        result: InstructionResult::PrecompileOOG,
                        gas: Gas::new(gas_limit),
                        output: alloy_primitives::Bytes::new(),
                    };

                    return Ok(Some(result));
                }

                // for each precompile and input we store in upper_gas_limit the minimum gas for
                // which we obtained a success, any gas limit above that value with succeed with
                // the same response, we can use it from the cache.
                if gas_limit >= entry.upper_gas_limit {
                    self.increment_by_one_precompile_cache_hits();

                    // for successful results, we need to ensure gas costs are correct when
                    // gas_limit differs. we only do this for successful results because it is the
                    // only case in which the inner precompile provider records gas costs.
                    if let Ok(mut result) = entry.result.clone() {
                        if result.result == InstructionResult::Return {
                            let mut adjusted_gas = Gas::new(gas_limit);
                            adjusted_gas.set_spent(result.gas.spent());

                            result.gas = adjusted_gas;
                        }

                        return Ok(Some(result));
                    }

                    return entry.result.clone().map(Some);
                }
            }

            // call the precompile if cache miss
            let output =
                self.precompile_provider.run(context, address, inputs, is_static, gas_limit);

            match &output {
                Ok(Some(result)) => {
                    self.increment_by_one_precompile_cache_misses();
                    let previous_entry_count = self.cache_entry_count();

                    if result.result == InstructionResult::PrecompileOOG {
                        // oog error
                        if let Some(mut entry) = cache_result {
                            entry.lower_gas_limit = entry.lower_gas_limit.max(gas_limit);
                        } else {
                            cache.cache.insert(
                                key,
                                CacheEntry {
                                    result: Ok(result.clone()),
                                    upper_gas_limit: u64::MAX,
                                    lower_gas_limit: gas_limit,
                                },
                            );
                        }
                    } else if result.result == InstructionResult::Return {
                        // success
                        if let Some(mut entry) = cache_result {
                            entry.result = Ok(result.clone());
                            entry.upper_gas_limit = entry.upper_gas_limit.min(gas_limit);
                        } else {
                            cache.cache.insert(
                                key,
                                CacheEntry {
                                    result: Ok(result.clone()),
                                    upper_gas_limit: gas_limit,
                                    lower_gas_limit: 0,
                                },
                            );
                        }
                    } else {
                        // for other errors cache the result
                        cache.cache.insert(
                            key,
                            CacheEntry {
                                result: Ok(result.clone()),
                                upper_gas_limit: gas_limit,
                                lower_gas_limit: 0,
                            },
                        );
                    }
                    self.update_precompile_cache_size(previous_entry_count);
                }
                Err(err) => {
                    // fatal error
                    self.increment_by_one_precompile_cache_misses();
                    let previous_entry_count = self.cache_entry_count();

                    cache.cache.insert(
                        key,
                        CacheEntry {
                            result: Err(err.clone()),
                            upper_gas_limit: gas_limit,
                            lower_gas_limit: 0,
                        },
                    );
                    self.update_precompile_cache_size(previous_entry_count);
                }
                Ok(None) => {
                    // precompile not found in inner provider
                }
            }

            output
        } else {
            self.precompile_provider.run(context, address, inputs, is_static, gas_limit)
        }
    }

    fn warm_addresses(&self) -> Box<impl Iterator<Item = Address>> {
        self.precompile_provider.warm_addresses()
    }

    fn contains(&self, address: &Address) -> bool {
        self.precompile_provider.contains(address)
    }
}

#[allow(dead_code, unused_variables)]
impl<P> MaybeCachedPrecompileProvider<P> {
    fn increment_by_one_precompile_cache_hits(&self) {
        self.metrics.precompile_cache_hits.increment(1);
    }

    fn increment_by_one_precompile_cache_misses(&self) {
        self.metrics.precompile_cache_misses.increment(1);
    }

    fn cache_entry_count(&self) -> u64 {
        self.cache.as_ref().map_or(0, |cache| cache.cache.entry_count())
    }

    fn update_precompile_cache_size(&self, previous_entry_count: u64) {
        {
            let new_entry_count = self.cache.as_ref().map_or(0, |cache| cache.cache.entry_count());
            if new_entry_count > previous_entry_count {
                self.metrics
                    .precompile_cache_size
                    .increment((new_entry_count - previous_entry_count) as f64);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_revm::revm::interpreter::{Gas, InstructionResult};

    #[test]
    fn test_precompile_cache_basic() {
        let cache = PrecompileCache::default();

        let key = PrecompileKey(b"test_input".into());

        let result = Ok(InterpreterResult::new(
            InstructionResult::Return,
            alloy_primitives::Bytes::copy_from_slice(b"cached_result"),
            Gas::new(50),
        ));

        let expected = CacheEntry { result, upper_gas_limit: 100, lower_gas_limit: 100 };
        cache.cache.insert(key.clone(), expected.clone());

        let actual = cache.cache.get(&key).unwrap();

        assert_eq!(actual, expected);
    }
}
