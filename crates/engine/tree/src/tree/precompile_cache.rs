//! Precompile cache backed by [`fixed_cache::Cache`] (lock-free, fixed-size).

use alloy_primitives::{map::FbBuildHasher};
use reth_evm::precompiles::{DynPrecompile, Precompile, PrecompileInput};
use reth_primitives_traits::dashmap::DashMap;
use revm::precompile::{PrecompileId, PrecompileOutput, PrecompileResult};
use revm_primitives::{map::DefaultHashBuilder, Address};
use std::{fmt, hash::Hash, sync::Arc};

/// Default number of cache entries per precompile.
const DEFAULT_CACHE_ENTRIES: usize = 1 << 14; // 16384

/// Fixed-size cache type used for precompile results.
type PrecompileFixedCache<S> = fixed_cache::Cache<Vec<u8>, CacheEntry<S>, DefaultHashBuilder>;

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
    pub fn cache_for_address(&self, address: Address) -> PrecompileCache<S> {
        // Try just using `.get` first to avoid acquiring a write lock.
        if let Some(cache) = self.0.get(&address) {
            return cache.clone();
        }
        // Otherwise, fallback to `.entry` and initialize the cache.
        //
        // This should be very rare as caches for all precompiles will be initialized as soon as
        // first EVM is created.
        self.0.entry(address).or_default().clone()
    }
}

/// Cache for precompiles, for each input stores the result.
///
/// Internally backed by [`fixed_cache::Cache`] behind an `Arc` so that multiple
/// [`CachedPrecompile`] instances for the same address share state.
#[derive(Clone)]
pub struct PrecompileCache<S>(Arc<PrecompileFixedCache<S>>)
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static;

impl<S> fmt::Debug for PrecompileCache<S>
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrecompileCache").finish()
    }
}

impl<S> Default for PrecompileCache<S>
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static,
{
    fn default() -> Self {
        Self(Arc::new(PrecompileFixedCache::new(DEFAULT_CACHE_ENTRIES, Default::default())))
    }
}

impl<S> PrecompileCache<S>
where
    S: Eq + Hash + fmt::Debug + Send + Sync + Clone + 'static,
{
    fn get(&self, input: &[u8], spec: S) -> Option<CacheEntry<S>> {
        self.0.get(input).filter(|e| e.spec == spec)
    }

    /// Inserts the given key and value into the cache.
    fn insert(&self, input: &[u8], value: CacheEntry<S>) {
        self.0.insert(input.to_vec(), value);
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
        if let Some(entry) = &self.cache.get(input.data, self.spec_id.clone()) {
            self.increment_by_one_precompile_cache_hits();
            if input.gas >= entry.gas_used() {
                return entry.to_precompile_result()
            }
        }

        let calldata = input.data;
        let result = self.precompile.call(input);

        match &result {
            Ok(output) => {
                self.cache.insert(
                    calldata,
                    CacheEntry { output: output.clone(), spec: self.spec_id.clone() },
                );
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
}
