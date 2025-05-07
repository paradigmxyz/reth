//! Contains a precompile cache that is backed by a moka cache.

use alloy_primitives::Bytes;
use reth_evm::precompiles::{DynPrecompile, Precompile};
use revm::precompile::{PrecompileError, PrecompileResult};
use std::sync::Arc;

/// Cache for precompiles, for each input stores the result.
pub type PrecompileCache =
    mini_moka::sync::Cache<CacheKey, CacheEntry, alloy_primitives::map::DefaultHashBuilder>;

/// Create a new [`PrecompileCache`].
pub fn create_precompile_cache() -> PrecompileCache {
    mini_moka::sync::CacheBuilder::new(100_000)
        .build_with_hasher(alloy_primitives::map::DefaultHashBuilder::default())
}

/// Cache key, just input for each precompile.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey(alloy_primitives::Bytes);

/// Combined entry containing both the result and gas bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    /// The actual result of executing the precompile.
    result: PrecompileResult,
    /// Observed gas used on successful run
    gas_used: u64,
    /// Gas limit observed on call error.
    err_gas_limit: u64,
}

/// A cache for precompile inputs / outputs.
#[derive(Debug)]
pub(crate) struct CachedPrecompile {
    /// Cache for precompile results and gas bounds.
    cache: Arc<PrecompileCache>,
    /// The precompile.
    precompile: DynPrecompile,
    /// Cache metrics.
    metrics: CachedPrecompileMetrics,
}

impl CachedPrecompile {
    /// `CachedPrecompile` constructor.
    pub(crate) fn new(precompile: DynPrecompile, cache: Arc<PrecompileCache>) -> Self {
        Self { precompile, cache, metrics: Default::default() }
    }

    pub(crate) fn wrap(precompile: DynPrecompile, cache: Arc<PrecompileCache>) -> DynPrecompile {
        let wrapped = Self::new(precompile, cache);
        move |data: &Bytes, gas_limit: u64| -> PrecompileResult { wrapped.call(data, gas_limit) }
            .into()
    }

    fn increment_by_one_precompile_cache_hits(&self) {
        self.metrics.precompile_cache_hits.increment(1);
    }

    fn increment_by_one_precompile_cache_misses(&self) {
        self.metrics.precompile_cache_misses.increment(1);
    }

    fn cache_size(&self) -> u64 {
        self.cache.weighted_size()
    }

    fn update_precompile_cache_size(&self, previous_size: u64) {
        let new_size = self.cache.weighted_size();
        if new_size > previous_size {
            self.metrics.precompile_cache_size.increment((new_size - previous_size) as f64);
        }
    }
}

impl Precompile for CachedPrecompile {
    fn call(&self, data: &Bytes, gas_limit: u64) -> PrecompileResult {
        let key = CacheKey(data.clone());

        let cache_result = self.cache.get(&key);

        if let Some(ref entry) = cache_result {
            match &entry.result {
                Ok(_) => {
                    self.increment_by_one_precompile_cache_hits();
                    if gas_limit < entry.gas_used {
                        return Err(PrecompileError::OutOfGas)
                    }
                    return entry.result.clone()
                }
                Err(PrecompileError::OutOfGas) => {
                    if gas_limit <= entry.err_gas_limit {
                        // entry.err_gas_limit gave OOG previously, anything equal or below
                        // should fail with OOG too.
                        self.increment_by_one_precompile_cache_hits();
                        return Err(PrecompileError::OutOfGas)
                    }
                }
                err => {
                    if gas_limit > entry.err_gas_limit {
                        // entry.err_gas_limit did not give OOG previously (it gave `err`),
                        // anything equal or above should not give it either
                        self.increment_by_one_precompile_cache_hits();
                        return err.clone()
                    }
                }
            }
        }

        // cache miss or error with not yet seen gas limit, call the precompile
        self.increment_by_one_precompile_cache_misses();
        let result = self.precompile.call(data, gas_limit);

        let previous_cache_size = self.cache_size();
        match &result {
            Ok(val) => {
                self.cache.insert(
                    key,
                    CacheEntry { result: result.clone(), gas_used: val.gas_used, err_gas_limit: 0 },
                );
            }
            _ => {
                self.cache.insert(
                    key,
                    CacheEntry { result: result.clone(), gas_used: 0, err_gas_limit: gas_limit },
                );
            }
        }

        self.update_precompile_cache_size(previous_cache_size);
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
    /// NOTE: this uses the moka caches`weighted_size` method to calculate size.
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

        let cache = CachedPrecompile::new(dyn_precompile, Arc::new(create_precompile_cache()));

        let key = CacheKey(b"test_input".into());

        let result = Ok(PrecompileOutput {
            gas_used: 50,
            bytes: alloy_primitives::Bytes::copy_from_slice(b"cached_result"),
        });

        let expected = CacheEntry { result, gas_used: 100, err_gas_limit: 100 };
        cache.cache.insert(key.clone(), expected.clone());

        let actual = cache.cache.get(&key).unwrap();

        assert_eq!(actual, expected);
    }
}
