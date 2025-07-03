//! Contains a precompile cache that is backed by a moka cache.

use alloy_primitives::Bytes;
use parking_lot::Mutex;
use reth_evm::precompiles::{DynPrecompile, Precompile, PrecompileInput};
use revm::precompile::{PrecompileOutput, PrecompileResult};
use revm_primitives::Address;
use schnellru::LruMap;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};

/// Default max cache size for [`PrecompileCache`]
const MAX_CACHE_SIZE: u32 = 10_000;

/// Stores caches for each precompile.
#[derive(Debug, Clone, Default)]
pub struct PrecompileCacheMap<S>(HashMap<Address, PrecompileCache<S>>)
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone;

impl<S> PrecompileCacheMap<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    pub(crate) fn cache_for_address(&mut self, address: Address) -> PrecompileCache<S> {
        self.0.entry(address).or_default().clone()
    }
}

/// Cache for precompiles, for each input stores the result.
///
/// [`LruMap`] requires a mutable reference on `get` since it updates the LRU order,
/// so we use a [`Mutex`] instead of an `RwLock`.
#[derive(Debug, Clone)]
pub struct PrecompileCache<S>(Arc<Mutex<LruMap<CacheKey<S>, CacheEntry>>>)
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone;

impl<S> Default for PrecompileCache<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn default() -> Self {
        Self(Arc::new(Mutex::new(LruMap::new(schnellru::ByLength::new(MAX_CACHE_SIZE)))))
    }
}

impl<S> PrecompileCache<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn get(&self, key: &CacheKeyRef<'_, S>) -> Option<CacheEntry> {
        self.0.lock().get(key).cloned()
    }

    /// Inserts the given key and value into the cache, returning the new cache size.
    fn insert(&self, key: CacheKey<S>, value: CacheEntry) -> usize {
        let mut cache = self.0.lock();
        cache.insert(key, value);
        cache.len()
    }
}

/// Cache key, spec id and precompile call input. spec id is included in the key to account for
/// precompile repricing across fork activations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey<S>((S, Bytes));

impl<S> CacheKey<S> {
    const fn new(spec_id: S, input: Bytes) -> Self {
        Self((spec_id, input))
    }
}

/// Cache key reference, used to avoid cloning the input bytes when looking up using a [`CacheKey`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKeyRef<'a, S>((S, &'a [u8]));

impl<'a, S> CacheKeyRef<'a, S> {
    const fn new(spec_id: S, input: &'a [u8]) -> Self {
        Self((spec_id, input))
    }
}

impl<S: PartialEq> PartialEq<CacheKey<S>> for CacheKeyRef<'_, S> {
    fn eq(&self, other: &CacheKey<S>) -> bool {
        self.0 .0 == other.0 .0 && self.0 .1 == other.0 .1.as_ref()
    }
}

impl<'a, S: Hash> Hash for CacheKeyRef<'a, S> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0 .0.hash(state);
        self.0 .1.hash(state);
    }
}

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
pub(crate) struct CachedPrecompile<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
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
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    /// `CachedPrecompile` constructor.
    pub(crate) const fn new(
        precompile: DynPrecompile,
        cache: PrecompileCache<S>,
        spec_id: S,
        metrics: Option<CachedPrecompileMetrics>,
    ) -> Self {
        Self { precompile, cache, spec_id, metrics }
    }

    pub(crate) fn wrap(
        precompile: DynPrecompile,
        cache: PrecompileCache<S>,
        spec_id: S,
        metrics: Option<CachedPrecompileMetrics>,
    ) -> DynPrecompile {
        let wrapped = Self::new(precompile, cache, spec_id, metrics);
        move |input: PrecompileInput<'_>| -> PrecompileResult { wrapped.call(input) }.into()
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
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn call(&self, input: PrecompileInput<'_>) -> PrecompileResult {
        let key = CacheKeyRef::new(self.spec_id.clone(), input.data);

        if let Some(entry) = &self.cache.get(&key) {
            self.increment_by_one_precompile_cache_hits();
            if input.gas >= entry.gas_used() {
                return entry.to_precompile_result()
            }
        }

        let calldata = input.data;
        let result = self.precompile.call(input);

        match &result {
            Ok(output) => {
                let key = CacheKey::new(self.spec_id.clone(), Bytes::copy_from_slice(calldata));
                let size = self.cache.insert(key, CacheEntry(output.clone()));
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
pub(crate) struct CachedPrecompileMetrics {
    /// Precompile cache hits
    precompile_cache_hits: metrics::Counter,

    /// Precompile cache misses
    precompile_cache_misses: metrics::Counter,

    /// Precompile cache size. Uses the LRU cache length as the size metric.
    precompile_cache_size: metrics::Gauge,

    /// Precompile execution errors.
    precompile_errors: metrics::Counter,
}

impl CachedPrecompileMetrics {
    /// Creates a new instance of [`CachedPrecompileMetrics`] with the given address.
    ///
    /// Adds address as an `address` label padded with zeros to at least two hex symbols, prefixed
    /// by `0x`.
    pub(crate) fn new_with_address(address: Address) -> Self {
        Self::new_with_labels(&[("address", format!("0x{address:02x}"))])
    }
}

#[cfg(test)]
mod tests {
    use std::hash::DefaultHasher;

    use super::*;
    use reth_evm::{EthEvmFactory, Evm, EvmEnv, EvmFactory};
    use reth_revm::db::EmptyDB;
    use revm::{context::TxEnv, precompile::PrecompileOutput};
    use revm_primitives::hardfork::SpecId;

    #[test]
    fn test_cache_key_ref_hash() {
        let key1 = CacheKey::new(SpecId::PRAGUE, b"test_input".into());
        let key2 = CacheKeyRef::new(SpecId::PRAGUE, b"test_input");
        assert!(PartialEq::eq(&key2, &key1));

        let mut hasher = DefaultHasher::new();
        key1.hash(&mut hasher);
        let hash1 = hasher.finish();

        let mut hasher = DefaultHasher::new();
        key2.hash(&mut hasher);
        let hash2 = hasher.finish();

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_precompile_cache_basic() {
        let dyn_precompile: DynPrecompile = |_input: PrecompileInput<'_>| -> PrecompileResult {
            Ok(PrecompileOutput { gas_used: 0, bytes: Bytes::default() })
        }
        .into();

        let cache =
            CachedPrecompile::new(dyn_precompile, PrecompileCache::default(), SpecId::PRAGUE, None);

        let output = PrecompileOutput {
            gas_used: 50,
            bytes: alloy_primitives::Bytes::copy_from_slice(b"cached_result"),
        };

        let key = CacheKey::new(SpecId::PRAGUE, b"test_input".into());
        let expected = CacheEntry(output);
        cache.cache.insert(key, expected.clone());

        let key = CacheKeyRef::new(SpecId::PRAGUE, b"test_input");
        let actual = cache.cache.get(&key).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_precompile_cache_map_separate_addresses() {
        let mut evm = EthEvmFactory::default().create_evm(EmptyDB::default(), EvmEnv::default());
        let input_data = b"same_input";
        let gas_limit = 100_000;

        let address1 = Address::repeat_byte(1);
        let address2 = Address::repeat_byte(2);

        let mut cache_map = PrecompileCacheMap::default();

        // create the first precompile with a specific output
        let precompile1: DynPrecompile = {
            move |input: PrecompileInput<'_>| -> PrecompileResult {
                assert_eq!(input.data, input_data);

                Ok(PrecompileOutput {
                    gas_used: 5000,
                    bytes: alloy_primitives::Bytes::copy_from_slice(b"output_from_precompile_1"),
                })
            }
        }
        .into();

        // create the second precompile with a different output
        let precompile2: DynPrecompile = {
            move |input: PrecompileInput<'_>| -> PrecompileResult {
                assert_eq!(input.data, input_data);

                Ok(PrecompileOutput {
                    gas_used: 7000,
                    bytes: alloy_primitives::Bytes::copy_from_slice(b"output_from_precompile_2"),
                })
            }
        }
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
}
