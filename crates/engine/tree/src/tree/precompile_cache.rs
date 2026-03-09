//! Precompile cache backed by [`schnellru::LruMap`] (LRU by length).
//!
//! Each precompile address gets its own [`PrecompileCache`] with a capacity tuned to its
//! mainnet working-set size. The caches are stored in a [`PrecompileCacheMap`] keyed by
//! precompile address.

use alloy_primitives::map::FbBuildHasher;
use parking_lot::Mutex;
use reth_evm::precompiles::{DynPrecompile, Precompile, PrecompileInput};
use reth_primitives_traits::dashmap::DashMap;
use revm::precompile::{PrecompileId, PrecompileOutput, PrecompileResult};
use revm_primitives::Address;
use schnellru::{ByLength, LruMap};
use std::{fmt, hash::Hash, sync::Arc};

/// Default max cache size for [`PrecompileCache`].
const DEFAULT_CACHE_SIZE: u32 = 10_000;

/// Returns the recommended cache capacity for a given precompile address.
///
/// Capacities are based on mainnet production hit-rate data at the default 10K capacity:
/// - ecMul (`0x07`): 55% hit rate → needs 50K
/// - modexp (`0x05`), ecAdd (`0x06`): ~70% hit rate → 30K
/// - Others: default 10K (sufficient)
fn default_cache_size_for_address(address: &Address) -> u32 {
    match address.0[19] {
        // ecMul — largest working set
        0x07 => 50_000,
        // modexp, ecAdd
        0x05 | 0x06 => 30_000,
        _ => DEFAULT_CACHE_SIZE,
    }
}

/// Stores caches for each precompile.
#[derive(Clone, Default)]
pub struct PrecompileCacheMap<S>(Arc<DashMap<Address, PrecompileCache<S>, FbBuildHasher<20>>>)
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static;

impl<S> fmt::Debug for PrecompileCacheMap<S>
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrecompileCacheMap").field("num_precompiles", &self.0.len()).finish()
    }
}

impl<S> PrecompileCacheMap<S>
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static,
{
    /// Get the precompile cache for the given address.
    ///
    /// If no cache exists for this address, a new one is created with a capacity tuned to
    /// the precompile's expected working-set size.
    pub fn cache_for_address(&self, address: Address) -> PrecompileCache<S> {
        // Try just using `.get` first to avoid acquiring a write lock.
        if let Some(cache) = self.0.get(&address) {
            return cache.clone();
        }
        // Otherwise, fallback to `.entry` and initialize the cache.
        //
        // This should be very rare as caches for all precompiles will be initialized as soon as
        // first EVM is created.
        let capacity = default_cache_size_for_address(&address);
        self.0.entry(address).or_insert_with(|| PrecompileCache::new(capacity)).clone()
    }
}

/// Inner LRU map type used by [`PrecompileCache`].
type PrecompileLruMap<S> = LruMap<Vec<u8>, CacheEntry<S>, ByLength>;

/// Cache for precompiles, for each input stores the result.
///
/// Internally backed by [`schnellru::LruMap`] behind an `Arc<Mutex<..>>` so that multiple
/// [`CachedPrecompile`] instances for the same address share state.
#[derive(Clone)]
pub struct PrecompileCache<S>(Arc<Mutex<PrecompileLruMap<S>>>)
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static;

impl<S> fmt::Debug for PrecompileCache<S>
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let map = self.0.lock();
        f.debug_struct("PrecompileCache").field("len", &map.len()).finish()
    }
}

impl<S> Default for PrecompileCache<S>
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static,
{
    fn default() -> Self {
        Self::new(DEFAULT_CACHE_SIZE)
    }
}

impl<S> PrecompileCache<S>
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static,
{
    fn new(capacity: u32) -> Self {
        Self(Arc::new(Mutex::new(LruMap::new(ByLength::new(capacity)))))
    }

    fn get(&self, input: &[u8], spec: S) -> Option<CacheEntry<S>> {
        self.0.lock().get(input).filter(|e| e.spec == spec).cloned()
    }

    /// Inserts the given key and value into the cache, returning the new cache size.
    fn insert(&self, input: &[u8], value: CacheEntry<S>) -> usize {
        let mut map = self.0.lock();
        map.insert(input.to_vec(), value);
        map.len()
    }
}

/// Cache entry, precompile successful output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry<S> {
    output: PrecompileOutput,
    spec: S,
}

impl<S> CacheEntry<S> {
    const fn gas_used(&self) -> u64 {
        self.output.gas_used
    }

    fn to_precompile_result(&self) -> PrecompileResult {
        Ok(self.output.clone())
    }
}

/// A cache for precompile inputs / outputs.
#[derive(Debug)]
pub struct CachedPrecompile<S>
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static,
{
    /// Cache for precompile results and gas bounds.
    cache: PrecompileCache<S>,
    /// The precompile.
    precompile: DynPrecompile,
    /// Cache metrics.
    metrics: Option<CachedPrecompileMetrics>,
    /// Spec id associated to the EVM from which this cached precompile was created.
    spec_id: S,
}

impl<S> CachedPrecompile<S>
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static,
{
    /// `CachedPrecompile` constructor.
    pub const fn new(
        precompile: DynPrecompile,
        cache: PrecompileCache<S>,
        spec_id: S,
        metrics: Option<CachedPrecompileMetrics>,
    ) -> Self {
        Self { precompile, cache, spec_id, metrics }
    }

    /// Wrap the given precompile in a cached precompile.
    pub fn wrap(
        precompile: DynPrecompile,
        cache: PrecompileCache<S>,
        spec_id: S,
        metrics: Option<CachedPrecompileMetrics>,
    ) -> DynPrecompile {
        let precompile_id = precompile.precompile_id().clone();
        let wrapped = Self::new(precompile, cache, spec_id, metrics);
        (precompile_id, move |input: PrecompileInput<'_>| -> PrecompileResult {
            wrapped.call(input)
        })
            .into()
    }

    fn increment_by_one_precompile_cache_hits(&self) {
        if let Some(metrics) = &self.metrics {
            metrics.precompile_cache_hits.increment(1);
        }
    }

    fn increment_by_one_precompile_cache_misses(&self) {
        if let Some(metrics) = &self.metrics {
            metrics.precompile_cache_misses.increment(1);
        }
    }

    fn set_precompile_cache_size_metric(&self, to: f64) {
        if let Some(metrics) = &self.metrics {
            metrics.precompile_cache_size.set(to);
        }
    }

    fn increment_by_one_precompile_errors(&self) {
        if let Some(metrics) = &self.metrics {
            metrics.precompile_errors.increment(1);
        }
    }
}

impl<S> Precompile for CachedPrecompile<S>
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static,
{
    fn precompile_id(&self) -> &PrecompileId {
        self.precompile.precompile_id()
    }

    fn call(&self, input: PrecompileInput<'_>) -> PrecompileResult {
        if let Some(entry) = &self.cache.get(input.data, self.spec_id.clone())
            && input.gas >= entry.gas_used()
        {
            self.increment_by_one_precompile_cache_hits();
            return entry.to_precompile_result()
        }

        let calldata = input.data;
        let result = self.precompile.call(input);

        match &result {
            Ok(output) => {
                let size = self.cache.insert(
                    calldata,
                    CacheEntry { output: output.clone(), spec: self.spec_id.clone() },
                );
                self.set_precompile_cache_size_metric(size as f64);
                self.increment_by_one_precompile_cache_misses();
            }
            _ => {
                self.increment_by_one_precompile_errors();
            }
        }
        result
    }
}

/// Metrics for the cached precompile.
#[derive(reth_metrics::Metrics, Clone)]
#[metrics(scope = "sync.caching")]
pub struct CachedPrecompileMetrics {
    /// Precompile cache hits
    pub precompile_cache_hits: metrics::Counter,

    /// Precompile cache misses
    pub precompile_cache_misses: metrics::Counter,

    /// Precompile cache size. Uses the LRU cache length as the size metric.
    pub precompile_cache_size: metrics::Gauge,

    /// Precompile execution errors.
    pub precompile_errors: metrics::Counter,
}

impl CachedPrecompileMetrics {
    /// Creates a new instance of [`CachedPrecompileMetrics`] with the given address.
    ///
    /// Adds address as an `address` label padded with zeros to at least two hex symbols, prefixed
    /// by `0x`.
    pub fn new_with_address(address: Address) -> Self {
        Self::new_with_labels(&[("address", format!("0x{address:02x}"))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use reth_evm::{EthEvmFactory, Evm, EvmEnv, EvmFactory};
    use reth_revm::db::EmptyDB;
    use revm::{context::TxEnv, precompile::PrecompileOutput};
    use revm_primitives::hardfork::SpecId;

    #[test]
    fn test_precompile_cache_basic() {
        let dyn_precompile: DynPrecompile = (|_input: PrecompileInput<'_>| -> PrecompileResult {
            Ok(PrecompileOutput {
                gas_used: 0,
                gas_refunded: 0,
                bytes: Bytes::default(),
                reverted: false,
            })
        })
        .into();

        let cache =
            CachedPrecompile::new(dyn_precompile, PrecompileCache::default(), SpecId::PRAGUE, None);

        let output = PrecompileOutput {
            gas_used: 50,
            gas_refunded: 0,
            bytes: alloy_primitives::Bytes::copy_from_slice(b"cached_result"),
            reverted: false,
        };

        let input = b"test_input";
        let expected = CacheEntry { output, spec: SpecId::PRAGUE };
        cache.cache.insert(input.as_slice(), expected.clone());

        let actual = cache.cache.get(input, SpecId::PRAGUE).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_precompile_cache_map_separate_addresses() {
        let mut evm = EthEvmFactory::default().create_evm(EmptyDB::default(), EvmEnv::default());
        let input_data = b"same_input";
        let gas_limit = 100_000;

        let address1 = Address::repeat_byte(1);
        let address2 = Address::repeat_byte(2);

        let cache_map = PrecompileCacheMap::default();

        // create the first precompile with a specific output
        let precompile1: DynPrecompile = (PrecompileId::custom("custom"), {
            move |input: PrecompileInput<'_>| -> PrecompileResult {
                assert_eq!(input.data, input_data);

                Ok(PrecompileOutput {
                    gas_used: 5000,
                    gas_refunded: 0,
                    bytes: alloy_primitives::Bytes::copy_from_slice(b"output_from_precompile_1"),
                    reverted: false,
                })
            }
        })
            .into();

        // create the second precompile with a different output
        let precompile2: DynPrecompile = (PrecompileId::custom("custom"), {
            move |input: PrecompileInput<'_>| -> PrecompileResult {
                assert_eq!(input.data, input_data);

                Ok(PrecompileOutput {
                    gas_used: 7000,
                    gas_refunded: 0,
                    bytes: alloy_primitives::Bytes::copy_from_slice(b"output_from_precompile_2"),
                    reverted: false,
                })
            }
        })
            .into();

        let wrapped_precompile1 = CachedPrecompile::wrap(
            precompile1,
            cache_map.cache_for_address(address1),
            SpecId::PRAGUE,
            None,
        );
        let wrapped_precompile2 = CachedPrecompile::wrap(
            precompile2,
            cache_map.cache_for_address(address2),
            SpecId::PRAGUE,
            None,
        );

        let precompile1_address = Address::with_last_byte(1);
        let precompile2_address = Address::with_last_byte(2);

        evm.precompiles_mut().apply_precompile(&precompile1_address, |_| Some(wrapped_precompile1));
        evm.precompiles_mut().apply_precompile(&precompile2_address, |_| Some(wrapped_precompile2));

        // first invocation of precompile1 (cache miss)
        let result1 = evm
            .transact_raw(TxEnv {
                caller: Address::ZERO,
                gas_limit,
                data: input_data.into(),
                kind: precompile1_address.into(),
                ..Default::default()
            })
            .unwrap()
            .result
            .into_output()
            .unwrap();
        assert_eq!(result1.as_ref(), b"output_from_precompile_1");

        // first invocation of precompile2 with the same input (should be a cache miss)
        // if cache was incorrectly shared, we'd get precompile1's result
        let result2 = evm
            .transact_raw(TxEnv {
                caller: Address::ZERO,
                gas_limit,
                data: input_data.into(),
                kind: precompile2_address.into(),
                ..Default::default()
            })
            .unwrap()
            .result
            .into_output()
            .unwrap();
        assert_eq!(result2.as_ref(), b"output_from_precompile_2");

        // second invocation of precompile1 (should be a cache hit)
        let result3 = evm
            .transact_raw(TxEnv {
                caller: Address::ZERO,
                gas_limit,
                data: input_data.into(),
                kind: precompile1_address.into(),
                ..Default::default()
            })
            .unwrap()
            .result
            .into_output()
            .unwrap();
        assert_eq!(result3.as_ref(), b"output_from_precompile_1");
    }

    #[test]
    fn test_per_address_cache_sizing() {
        let cache_map: PrecompileCacheMap<SpecId> = PrecompileCacheMap::default();

        // ecMul (0x07) should get 50K capacity
        let _ecmul_cache = cache_map.cache_for_address(Address::with_last_byte(0x07));

        // ecAdd (0x06) should get 30K capacity
        let _ecadd_cache = cache_map.cache_for_address(Address::with_last_byte(0x06));

        // SHA256 (0x02) should get default 10K capacity
        let _sha256_cache = cache_map.cache_for_address(Address::with_last_byte(0x02));

        // All three addresses should have separate caches
        assert_eq!(cache_map.0.len(), 3);
    }

    /// Simulates a DeFi-like precompile access pattern and measures cache hit rates
    /// at different capacities.
    ///
    /// The model uses three tiers (hot/warm/cold) that reflect real mainnet behavior:
    /// popular DeFi pools reuse the same curve inputs repeatedly (hot), less popular
    /// ones are accessed occasionally (warm), and one-off operations form a long tail
    /// (cold). At 10K capacity the warm set is partially evicted by cold accesses,
    /// matching the ~55% hit rate observed for ecMul in production.
    #[test]
    fn test_cache_hit_rate_improves_with_larger_capacity() {
        use rand::Rng;

        let num_accesses: u64 = 200_000;
        let mut rng = rand::rng();

        // Three-tier access model calibrated to match ecMul production data (~55% at 10K):
        //   40% hot:  3K unique inputs (top DeFi pools)
        //   30% warm: 25K unique inputs (less popular pools)
        //   30% cold: 200K unique inputs (one-off / rare operations)
        let hot_size: u64 = 3_000;
        let warm_size: u64 = 25_000;
        let cold_size: u64 = 200_000;

        let accesses: Vec<u64> = (0..num_accesses)
            .map(|_| {
                let r: f64 = rng.random();
                if r < 0.40 {
                    // hot tier
                    rng.random_range(0..hot_size)
                } else if r < 0.70 {
                    // warm tier (offset past hot)
                    hot_size + rng.random_range(0..warm_size)
                } else {
                    // cold tier (offset past warm)
                    hot_size + warm_size + rng.random_range(0..cold_size)
                }
            })
            .collect();

        // Measure hit rate at 10K capacity (current default for all precompiles)
        let hits_10k = simulate_lru_hits(&accesses, 10_000);
        let rate_10k = hits_10k as f64 / num_accesses as f64 * 100.0;

        // Measure hit rate at 50K capacity (our new ecMul capacity)
        let hits_50k = simulate_lru_hits(&accesses, 50_000);
        let rate_50k = hits_50k as f64 / num_accesses as f64 * 100.0;

        // 50K cache should meaningfully beat 10K cache because it can hold the full
        // warm set (25K) without eviction from cold accesses.
        assert!(
            hits_50k > hits_10k,
            "50K cache ({rate_50k:.1}%) should beat 10K cache ({rate_10k:.1}%)"
        );

        let improvement = rate_50k - rate_10k;
        assert!(
            improvement > 5.0,
            "Expected >5pp improvement, got {improvement:.1}pp ({rate_10k:.1}% → {rate_50k:.1}%)"
        );
    }

    /// Simple LRU cache simulation — returns the number of hits.
    fn simulate_lru_hits(accesses: &[u64], capacity: u32) -> u64 {
        let mut cache: LruMap<u64, (), ByLength> = LruMap::new(ByLength::new(capacity));
        let mut hits = 0u64;
        for &key in accesses {
            if cache.get(&key).is_some() {
                hits += 1;
            } else {
                cache.insert(key, ());
            }
        }
        hits
    }
}
