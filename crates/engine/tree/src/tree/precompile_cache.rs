//! Contains a precompile cache that is backed by a moka cache.

use alloy_primitives::Bytes;
use reth_evm::precompiles::{DynPrecompile, Precompile};
use revm::precompile::{PrecompileError, PrecompileResult};
use std::sync::Arc;

type Cache<K, V> = mini_moka::sync::Cache<K, V, alloy_primitives::map::DefaultHashBuilder>;

/// Cache key, just input for each precompile.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PrecompileKey(alloy_primitives::Bytes);

/// Combined entry containing both the result and gas bounds.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CacheEntry {
    /// The actual result of executing the precompile.
    result: PrecompileResult,
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
pub(crate) struct CachedPrecompile {
    /// Cache for precompile results and gas bounds.
    cache: Arc<Cache<PrecompileKey, CacheEntry>>,
    /// The precompile.
    precompile: DynPrecompile,
    /// Cache metrics.
    metrics: CachedPrecompileMetrics,
}

impl CachedPrecompile {
    /// `CachedPrecompile` constructor.
    pub(crate) fn new(
        precompile: DynPrecompile,
        cache: Arc<Cache<PrecompileKey, CacheEntry>>,
    ) -> Self {
        Self { precompile, cache, metrics: Default::default() }
    }

    fn increment_by_one_precompile_cache_hits(&self) {
        self.metrics.precompile_cache_hits.increment(1);
    }

    fn increment_by_one_precompile_cache_misses(&self) {
        self.metrics.precompile_cache_misses.increment(1);
    }

    fn cache_entry_count(&self) -> u64 {
        self.cache.entry_count()
    }

    fn update_precompile_cache_size(&self, previous_entry_count: u64) {
        let new_entry_count = self.cache.entry_count();
        if new_entry_count > previous_entry_count {
            self.metrics
                .precompile_cache_size
                .increment((new_entry_count - previous_entry_count) as f64);
        }
    }
}

impl Precompile for CachedPrecompile {
    fn call(&self, data: &Bytes, gas_limit: u64) -> PrecompileResult {
        let key = PrecompileKey(data.clone());

        let cache_result = self.cache.get(&key);

        if let Some(ref entry) = cache_result {
            // for each precompile and input we store in lower_gas_limit the maximum gas for
            // which we have received an out of gas error, any gas limit below that will fail
            // with OOG too.
            if gas_limit <= entry.lower_gas_limit {
                self.increment_by_one_precompile_cache_hits();

                return Err(PrecompileError::OutOfGas);
            }

            // for each precompile and input we store in upper_gas_limit the minimum gas for
            // which we obtained a success, any gas limit above that value with succeed with
            // the same response, we can use it from the cache.
            if gas_limit >= entry.upper_gas_limit {
                self.increment_by_one_precompile_cache_hits();

                // no need to change gas on successful results, on success the gas spent is the same
                // no matter gas the limit

                return entry.result.clone();
            }
        }

        // cache miss or unknown gas limit, call the precompile
        let result = self.precompile.call(data, gas_limit);

        let previous_entry_count = self.cache_entry_count();
        match &result {
            Ok(_) => {
                if let Some(mut entry) = cache_result {
                    // update cached result and upper gas limit
                    entry.result = result.clone();
                    entry.upper_gas_limit = entry.upper_gas_limit.min(gas_limit);
                } else {
                    self.increment_by_one_precompile_cache_misses();
                    self.cache.insert(
                        key,
                        CacheEntry {
                            result: result.clone(),
                            upper_gas_limit: gas_limit,
                            lower_gas_limit: 0,
                        },
                    );
                }
            }
            Err(PrecompileError::OutOfGas) => {
                if let Some(mut entry) = cache_result {
                    // update cached result and lower gas limit
                    entry.result = result.clone();
                    entry.lower_gas_limit = entry.lower_gas_limit.max(gas_limit);
                } else {
                    self.increment_by_one_precompile_cache_misses();
                    self.cache.insert(
                        key,
                        CacheEntry {
                            result: result.clone(),
                            upper_gas_limit: u64::MAX,
                            lower_gas_limit: gas_limit,
                        },
                    );
                }
            }
            _ => {
                // for other errors, update the cached result or insert cache entry
                if let Some(mut entry) = cache_result {
                    entry.result = result.clone();
                } else {
                    self.increment_by_one_precompile_cache_misses();
                    self.cache.insert(
                        key,
                        CacheEntry {
                            result: result.clone(),
                            upper_gas_limit: gas_limit,
                            lower_gas_limit: 0,
                        },
                    );
                }
            }
        }

        self.update_precompile_cache_size(previous_entry_count);
        result
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

#[cfg(test)]
mod tests {
    use super::*;
    use revm::precompile::PrecompileOutput;

    #[test]
    fn test_precompile_cache_basic() {
        let dyn_precompile: DynPrecompile = |_input: &Bytes, _gas: u64| -> PrecompileResult {
            Ok(PrecompileOutput { gas_used: 0, bytes: Bytes::default() })
        }
        .into();

        let cache = CachedPrecompile::new(
            dyn_precompile,
            Arc::new(
                mini_moka::sync::CacheBuilder::new(100_000)
                    .build_with_hasher(alloy_primitives::map::DefaultHashBuilder::default()),
            ),
        );

        let key = PrecompileKey(b"test_input".into());

        let result = Ok(PrecompileOutput {
            gas_used: 50,
            bytes: alloy_primitives::Bytes::copy_from_slice(b"cached_result"),
        });

        let expected = CacheEntry { result, upper_gas_limit: 100, lower_gas_limit: 100 };
        cache.cache.insert(key.clone(), expected.clone());

        let actual = cache.cache.get(&key).unwrap();

        assert_eq!(actual, expected);
    }
}
