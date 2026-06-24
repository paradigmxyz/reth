//! Precompile cache for payload prewarming.

use alloy_primitives::{
    map::{DefaultHashBuilder, FbBuildHasher},
    Address, Bytes,
};
use evm2::{
    evm::precompile::{PrecompileOutput, PrecompileProvider},
    interpreter::{GasTracker, Message},
    BaseEvmTypes, Evm, PrecompileError,
};
use moka::policy::EvictionPolicy;
#[cfg(feature = "metrics")]
use reth_metrics::Metrics;
use reth_primitives_traits::dashmap::DashMap;
use std::{fmt, hash::Hash, sync::Arc};
use tracing::error;

/// Default max cache size for [`PrecompileCache`].
const MAX_CACHE_SIZE: u32 = 1024 * 1024;

/// Stores caches for each precompile.
pub struct PrecompileCacheMap<S>(Arc<DashMap<Address, PrecompileCache<S>, FbBuildHasher<20>>>);

/// EVM precompile cache keyed by [`evm2::SpecId`].
pub type SpecPrecompileCacheMap = PrecompileCacheMap<evm2::SpecId>;

impl<S> fmt::Debug for PrecompileCacheMap<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrecompileCacheMap").finish_non_exhaustive()
    }
}

impl<S> Clone for PrecompileCacheMap<S> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<S> Default for PrecompileCacheMap<S> {
    fn default() -> Self {
        Self(Arc::new(DashMap::with_hasher(Default::default())))
    }
}

impl<S> PrecompileCacheMap<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    /// Get the precompile cache for the given address.
    pub fn cache_for_address(&self, address: Address) -> PrecompileCache<S> {
        if let Some(cache) = self.0.get(&address) {
            return cache.clone()
        }

        self.0.entry(address).or_default().clone()
    }
}

/// Cache for one precompile's inputs and outputs.
pub struct PrecompileCache<S>(moka::sync::Cache<Bytes, CacheEntry<S>, DefaultHashBuilder>);

impl<S> fmt::Debug for PrecompileCache<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrecompileCache").finish_non_exhaustive()
    }
}

impl<S> Clone for PrecompileCache<S> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<S> Default for PrecompileCache<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn default() -> Self {
        Self(
            moka::sync::CacheBuilder::new(MAX_CACHE_SIZE as u64)
                .initial_capacity(MAX_CACHE_SIZE as usize)
                .eviction_policy(EvictionPolicy::lru())
                .weigher(|key: &Bytes, value: &CacheEntry<S>| {
                    (key.len() + value.output.bytes().len()) as u32
                })
                .build_with_hasher(Default::default()),
        )
    }
}

impl<S> PrecompileCache<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn get(&self, input: &[u8], spec: S) -> Option<CacheEntry<S>> {
        self.0.get(input).filter(|entry| entry.spec == spec)
    }

    fn insert(&self, input: Bytes, value: CacheEntry<S>) -> usize {
        self.0.insert(input, value);
        self.0.entry_count() as usize
    }
}

/// Cache entry for a successful precompile output.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheEntry<S> {
    output: PrecompileOutput,
    regular_gas_used: u64,
    spec: S,
}

impl<S> CacheEntry<S> {
    fn to_precompile_result(&self) -> PrecompileOutput {
        self.output.clone()
    }
}

/// A caching EVM precompile provider.
pub struct CachedPrecompileProvider<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    inner: evm2::Precompiles,
    cache_map: PrecompileCacheMap<S>,
    spec_id: S,
    #[cfg_attr(not(feature = "metrics"), allow(dead_code))]
    metrics: Option<CachedPrecompileMetrics>,
}

impl<S> fmt::Debug for CachedPrecompileProvider<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachedPrecompileProvider").finish_non_exhaustive()
    }
}

impl<S> CachedPrecompileProvider<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    /// Creates a new cached precompile provider.
    pub const fn new(
        inner: evm2::Precompiles,
        cache_map: PrecompileCacheMap<S>,
        spec_id: S,
        metrics: Option<CachedPrecompileMetrics>,
    ) -> Self {
        Self { inner, cache_map, spec_id, metrics }
    }

    #[cfg_attr(not(feature = "metrics"), allow(clippy::missing_const_for_fn))]
    fn increment_by_one_precompile_cache_hits(&self) {
        #[cfg(feature = "metrics")]
        if let Some(metrics) = &self.metrics {
            metrics.precompile_cache_hits.increment(1);
        }
    }

    #[cfg_attr(not(feature = "metrics"), allow(clippy::missing_const_for_fn))]
    fn increment_by_one_precompile_cache_misses(&self) {
        #[cfg(feature = "metrics")]
        if let Some(metrics) = &self.metrics {
            metrics.precompile_cache_misses.increment(1);
        }
    }

    #[cfg_attr(not(feature = "metrics"), allow(clippy::missing_const_for_fn))]
    fn set_precompile_cache_size_metric(&self, to: f64) {
        #[cfg(not(feature = "metrics"))]
        let _ = to;

        #[cfg(feature = "metrics")]
        if let Some(metrics) = &self.metrics {
            metrics.precompile_cache_size.set(to);
        }
    }

    #[cfg_attr(not(feature = "metrics"), allow(clippy::missing_const_for_fn))]
    fn increment_by_one_precompile_errors(&self) {
        #[cfg(feature = "metrics")]
        if let Some(metrics) = &self.metrics {
            metrics.precompile_errors.increment(1);
        }
    }
}

impl<S> PrecompileProvider<BaseEvmTypes> for CachedPrecompileProvider<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn addresses(&self) -> Vec<Address> {
        self.inner.addresses()
    }

    fn contains(&self, address: &Address) -> bool {
        self.inner.contains(address)
    }

    fn execute(
        &mut self,
        evm: &mut Evm<BaseEvmTypes>,
        message: &Message<BaseEvmTypes>,
        gas: &mut GasTracker,
    ) -> Option<Result<PrecompileOutput, PrecompileError>> {
        let address = message.code_address;
        let cache = self.cache_map.cache_for_address(address);

        if let Some(entry) = cache.get(message.input.as_ref(), self.spec_id.clone()) {
            return Some(match gas.spend(entry.regular_gas_used).map_err(PrecompileError::from) {
                Ok(()) => {
                    self.increment_by_one_precompile_cache_hits();
                    Ok(entry.to_precompile_result())
                }
                Err(err) => {
                    self.increment_by_one_precompile_errors();
                    Err(err)
                }
            })
        }

        let before = GasSnapshot::new(gas);
        let result = self.inner.execute(evm, message, gas)?;
        let after = GasSnapshot::new(gas);

        match &result {
            Ok(output) => {
                if before.reservoir != after.reservoir {
                    error!(
                        target: "evm::precompile_cache",
                        %address,
                        "cacheable precompile decremented reservoir, skipping cache insertion"
                    );
                } else if before.state_gas_spent != after.state_gas_spent {
                    error!(
                        target: "evm::precompile_cache",
                        %address,
                        "cacheable precompile used state gas, skipping cache insertion"
                    );
                } else if before.refunded != after.refunded {
                    error!(
                        target: "evm::precompile_cache",
                        %address,
                        "cacheable precompile changed refund gas, skipping cache insertion"
                    );
                } else if let Some(regular_gas_used) = after.spent.checked_sub(before.spent) {
                    let size = cache.insert(
                        Bytes::copy_from_slice(message.input.as_ref()),
                        CacheEntry {
                            output: output.clone(),
                            regular_gas_used,
                            spec: self.spec_id.clone(),
                        },
                    );
                    self.set_precompile_cache_size_metric(size as f64);
                    self.increment_by_one_precompile_cache_misses();
                } else {
                    error!(
                        target: "evm::precompile_cache",
                        %address,
                        "cacheable precompile returned regular gas, skipping cache insertion"
                    );
                }
            }
            Err(_) => {
                self.increment_by_one_precompile_errors();
            }
        }

        Some(result)
    }
}

#[derive(Debug)]
struct GasSnapshot {
    spent: u64,
    reservoir: u64,
    state_gas_spent: u64,
    refunded: i64,
}

impl GasSnapshot {
    const fn new(gas: &GasTracker) -> Self {
        Self {
            spent: gas.spent(),
            reservoir: gas.reservoir(),
            state_gas_spent: gas.state_gas_spent(),
            refunded: gas.refunded(),
        }
    }
}

/// Metrics for the cached precompile.
#[cfg(feature = "metrics")]
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.caching")]
pub struct CachedPrecompileMetrics {
    /// Precompile cache hits.
    pub precompile_cache_hits: metrics::Counter,

    /// Precompile cache misses.
    pub precompile_cache_misses: metrics::Counter,

    /// Precompile cache size. Uses the LRU cache length as the size metric.
    pub precompile_cache_size: metrics::Gauge,

    /// Precompile execution errors.
    pub precompile_errors: metrics::Counter,
}

/// Metrics for the cached precompile.
#[cfg(not(feature = "metrics"))]
#[derive(Debug, Clone)]
pub struct CachedPrecompileMetrics;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, B256, U256};
    use evm2::{
        env::BlockEnv,
        evm::{precompile::NoPrecompiles, InMemoryDB},
        interpreter::{Message, MessageKind},
        registry::TxRegistry,
        SpecId,
    };

    #[test]
    fn caches_successful_precompile_output() {
        let cache_map = PrecompileCacheMap::default();
        let mut provider = CachedPrecompileProvider::new(
            evm2::Precompiles::base(SpecId::OSAKA),
            cache_map.clone(),
            SpecId::OSAKA,
            None,
        );
        let mut evm = Evm::<BaseEvmTypes>::new(
            SpecId::OSAKA,
            BlockEnv::default(),
            TxRegistry::new(),
            InMemoryDB::default(),
            NoPrecompiles::default(),
        );
        let address = Address::with_last_byte(4);
        let message = Message {
            kind: MessageKind::Call,
            gas_limit: 30_000,
            destination: address,
            caller: Address::ZERO,
            input: Bytes::copy_from_slice(b"cached-input"),
            value: U256::ZERO,
            code_address: address,
            disable_precompiles: false,
            salt: B256::ZERO,
            ..Default::default()
        };

        let mut gas = GasTracker::new(30_000);
        let output = provider
            .execute(&mut evm, &message, &mut gas)
            .expect("identity precompile exists")
            .expect("identity precompile succeeds");
        assert_eq!(output.bytes(), b"cached-input");

        let cache = cache_map.cache_for_address(address);
        let entry = cache.get(message.input.as_ref(), SpecId::OSAKA).expect("cache entry exists");
        assert_eq!(entry.output.bytes(), b"cached-input");
        assert_eq!(entry.regular_gas_used, 18);

        let mut hit_gas = GasTracker::new(30_000);
        let output = provider
            .execute(&mut evm, &message, &mut hit_gas)
            .expect("identity precompile exists")
            .expect("cached identity precompile succeeds");
        assert_eq!(output.bytes(), b"cached-input");
        assert_eq!(hit_gas.spent(), 18);
    }
}
