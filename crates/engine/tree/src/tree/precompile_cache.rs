//! Contains a precompile cache that is backed by a moka cache.

use reth_evm::precompiles::{DynPrecompile, Precompile};
use revm::precompile::{PrecompileOutput, PrecompileResult};
use revm_primitives::{Address, Bytes};
use schnellru::LruMap;
use std::{collections::HashMap, hash::Hash, sync::Arc};

/// Stores caches for each precompile.
#[derive(Debug, Clone, Default)]
pub struct PrecompileCacheMap(HashMap<Address, PrecompileCache>);

impl PrecompileCacheMap {
    pub(crate) fn cache_for_address(&mut self, address: Address) -> PrecompileCache {
        self.0.entry(address).or_default().clone()
    }
}

/// Cache for precompiles, for each input stores the result.
#[derive(Debug, Clone)]
pub struct PrecompileCache(Arc<parking_lot::RwLock<LruMap<CacheKey, CacheEntry>>>);

impl Default for PrecompileCache {
    fn default() -> Self {
        Self(Arc::new(parking_lot::RwLock::new(LruMap::new(schnellru::ByLength::new(100_000)))))
    }
}

impl PrecompileCache {
    fn get(&self, key: &[u8]) -> Option<CacheEntry> {
        let lookup_key = CacheKey(Bytes::copy_from_slice(key));
        self.0.write().get(&lookup_key).cloned()
    }

    fn insert(&self, key: CacheKey, value: CacheEntry) {
        self.0.write().insert(key, value);
    }

    fn weighted_size(&self) -> u64 {
        self.0.read().len() as u64
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
        if let Some(entry) = self.cache.get(data) {
            self.increment_by_one_precompile_cache_hits();
            if gas_limit >= entry.gas_used() {
                return entry.to_precompile_result();
            }
        }

        let result = self.precompile.call(data, gas_limit);

        match &result {
            Ok(output) => {
                self.increment_by_one_precompile_cache_misses();
                let key = CacheKey(Bytes::copy_from_slice(data));
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
    /// Tracks the number of items currently stored in the precompile LRU cache.
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

        let actual = cache.cache.get(key.0.as_ref()).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_precompile_cache_map_separate_addresses() {
        let input_data = b"same_input";
        let gas_limit = 100_000;

        let address1 = Address::repeat_byte(1);
        let address2 = Address::repeat_byte(2);

        let mut cache_map = PrecompileCacheMap::default();

        // create the first precompile with a specific output
        let precompile1: DynPrecompile = {
            move |data: &[u8], _gas: u64| -> PrecompileResult {
                assert_eq!(data, input_data);

                Ok(PrecompileOutput {
                    gas_used: 5000,
                    bytes: alloy_primitives::Bytes::copy_from_slice(b"output_from_precompile_1"),
                })
            }
        }
        .into();

        // create the second precompile with a different output
        let precompile2: DynPrecompile = {
            move |data: &[u8], _gas: u64| -> PrecompileResult {
                assert_eq!(data, input_data);

                Ok(PrecompileOutput {
                    gas_used: 7000,
                    bytes: alloy_primitives::Bytes::copy_from_slice(b"output_from_precompile_2"),
                })
            }
        }
        .into();

        let wrapped_precompile1 =
            CachedPrecompile::wrap(precompile1, cache_map.cache_for_address(address1));
        let wrapped_precompile2 =
            CachedPrecompile::wrap(precompile2, cache_map.cache_for_address(address2));

        // first invocation of precompile1 (cache miss)
        let result1 = wrapped_precompile1.call(input_data, gas_limit).unwrap();
        assert_eq!(result1.bytes.as_ref(), b"output_from_precompile_1");

        // first invocation of precompile2 with the same input (should be a cache miss)
        // if cache was incorrectly shared, we'd get precompile1's result
        let result2 = wrapped_precompile2.call(input_data, gas_limit).unwrap();
        assert_eq!(result2.bytes.as_ref(), b"output_from_precompile_2");

        // second invocation of precompile1 (should be a cache hit)
        let result3 = wrapped_precompile1.call(input_data, gas_limit).unwrap();
        assert_eq!(result3.bytes.as_ref(), b"output_from_precompile_1");
    }
}
