//! Contains a precompile cache that is backed by a moka cache.

use alloy_primitives::map::Entry;
use reth_evm::precompiles::{DynPrecompile, Precompile};
use revm::precompile::{PrecompileOutput, PrecompileResult};
use revm_primitives::{Address, Bytes, HashMap};
use std::sync::Arc;

/// Stores caches for each precompile.
#[derive(Debug, Clone, Default)]
pub struct PrecompileCacheMap(HashMap<Address, PrecompileCache>);

impl PrecompileCacheMap {
    pub(crate) fn entry(&mut self, address: Address) -> Entry<'_, Address, PrecompileCache> {
        self.0.entry(address)
    }
}

/// Cache for precompiles, for each input stores the result.
#[derive(Debug, Clone)]
pub struct PrecompileCache(
    Arc<mini_moka::sync::Cache<CacheKey, CacheEntry, alloy_primitives::map::DefaultHashBuilder>>,
);

impl Default for PrecompileCache {
    fn default() -> Self {
        Self(Arc::new(
            mini_moka::sync::CacheBuilder::new(100_000)
                .build_with_hasher(alloy_primitives::map::DefaultHashBuilder::default()),
        ))
    }
}

impl PrecompileCache {
    fn get(&self, key: &CacheKey) -> Option<CacheEntry> {
        self.0.get(key)
    }

    fn insert(&self, key: CacheKey, value: CacheEntry) {
        self.0.insert(key, value);
    }

    fn weighted_size(&self) -> u64 {
        self.0.weighted_size()
    }
}

/// Cache key, precompile call input.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey(alloy_primitives::Bytes);

/// Cache entry, precompile successful output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry(PrecompileOutput);

impl CacheEntry {
    const fn gas_used(&self) -> u64 {
        self.0.gas_used
    }

    fn to_precompile_result(&self) -> PrecompileResult {
        Ok(self.0.clone())
    }
}

/// A cache for precompile inputs / outputs.
#[derive(Debug)]
pub(crate) struct CachedPrecompile {
    /// Cache for precompile results and gas bounds.
    cache: PrecompileCache,
    /// The precompile.
    precompile: DynPrecompile,
    /// Cache metrics.
    metrics: CachedPrecompileMetrics,
}

impl CachedPrecompile {
    /// `CachedPrecompile` constructor.
    pub(crate) fn new(precompile: DynPrecompile, cache: PrecompileCache) -> Self {
        Self { precompile, cache, metrics: Default::default() }
    }

    pub(crate) fn wrap(precompile: DynPrecompile, cache: PrecompileCache) -> DynPrecompile {
        let wrapped = Self::new(precompile, cache);
        move |data: &[u8], gas_limit: u64| -> PrecompileResult { wrapped.call(data, gas_limit) }
            .into()
    }

    fn increment_by_one_precompile_cache_hits(&self) {
        self.metrics.precompile_cache_hits.increment(1);
    }

    fn increment_by_one_precompile_cache_misses(&self) {
        self.metrics.precompile_cache_misses.increment(1);
    }

    fn increment_by_one_precompile_errors(&self) {
        self.metrics.precompile_errors.increment(1);
    }

    fn update_precompile_cache_size(&self) {
        let new_size = self.cache.weighted_size();
        self.metrics.precompile_cache_size.set(new_size as f64);
    }
}

impl Precompile for CachedPrecompile {
    fn call(&self, data: &[u8], gas_limit: u64) -> PrecompileResult {
        let key = CacheKey(Bytes::copy_from_slice(data));

        if let Some(entry) = &self.cache.get(&key) {
            self.increment_by_one_precompile_cache_hits();
            if gas_limit >= entry.gas_used() {
                return entry.to_precompile_result()
            }
        }

        let result = self.precompile.call(data, gas_limit);

        match &result {
            Ok(output) => {
                self.increment_by_one_precompile_cache_misses();
                self.cache.insert(key, CacheEntry(output.clone()));
            }
            _ => {
                self.increment_by_one_precompile_errors();
            }
        }

        self.update_precompile_cache_size();
        result
    }
}

/// Metrics for the cached precompile.
#[derive(reth_metrics::Metrics, Clone)]
#[metrics(scope = "sync.caching")]
pub(crate) struct CachedPrecompileMetrics {
    /// Precompile cache hits
    precompile_cache_hits: metrics::Counter,

    /// Precompile cache misses
    precompile_cache_misses: metrics::Counter,

    /// Precompile cache size
    ///
    /// NOTE: this uses the moka caches`weighted_size` method to calculate size.
    precompile_cache_size: metrics::Gauge,

    /// Precompile execution errors.
    precompile_errors: metrics::Counter,
}

#[cfg(test)]
mod tests {
    use super::*;
    use revm::precompile::PrecompileOutput;

    #[test]
    fn test_precompile_cache_basic() {
        let dyn_precompile: DynPrecompile = |_input: &[u8], _gas: u64| -> PrecompileResult {
            Ok(PrecompileOutput { gas_used: 0, bytes: Bytes::default() })
        }
        .into();

        let cache = CachedPrecompile::new(dyn_precompile, PrecompileCache::default());

        let key = CacheKey(b"test_input".into());

        let output = PrecompileOutput {
            gas_used: 50,
            bytes: alloy_primitives::Bytes::copy_from_slice(b"cached_result"),
        };

        let expected = CacheEntry(output);
        cache.cache.insert(key.clone(), expected.clone());

        let actual = cache.cache.get(&key).unwrap();

        assert_eq!(actual, expected);
    }
}
