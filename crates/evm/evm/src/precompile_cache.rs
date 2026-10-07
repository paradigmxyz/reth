//! Precompile cache for payload prewarming.

use alloy_primitives::{
    map::{AddressSet, DefaultHashBuilder, FbBuildHasher},
    Address, Bytes,
};
use evm2::{
    evm::precompile::{PrecompileOutput, PrecompileProvider},
    interpreter::{GasTracker, Message},
    Evm, EvmTypesHost, PrecompileError,
};
use moka::policy::EvictionPolicy;
#[cfg(feature = "metrics")]
use reth_metrics::Metrics;
use reth_primitives_traits::dashmap::DashMap;
use std::{fmt, hash::Hash, sync::Arc};
use tracing::error;

/// Default max cache size for [`PrecompileCache`].
const MAX_CACHE_SIZE: u32 = 1024 * 1024;

/// Maximum input retained by a precompile cache.
const MAX_PRECOMPILE_CACHE_INPUT_SIZE: usize = 2 * 1024;

/// Stores caches for each precompile.
pub struct PrecompileCacheMap<S>(Arc<DashMap<Address, Arc<PrecompileCache<S>>, FbBuildHasher<20>>>);

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
    fn cache_for_address(&self, address: Address) -> Arc<PrecompileCache<S>> {
        if let Some(cache) = self.0.get(&address) {
            return cache.clone()
        }

        self.0.entry(address).or_default().clone()
    }
}

/// Cache for one precompile's inputs and outputs.
struct PrecompileCache<S> {
    cache: moka::sync::Cache<Bytes, CacheEntry<S>, DefaultHashBuilder>,
    #[cfg(feature = "metrics")]
    metrics: std::sync::OnceLock<CachedPrecompileMetrics>,
}

impl<S> fmt::Debug for PrecompileCache<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrecompileCache").finish_non_exhaustive()
    }
}

impl<S> Default for PrecompileCache<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn default() -> Self {
        Self {
            cache: moka::sync::CacheBuilder::new(MAX_CACHE_SIZE as u64)
                .eviction_policy(EvictionPolicy::lru())
                .weigher(|key: &Bytes, value: &CacheEntry<S>| {
                    (key.len() + value.output.bytes().len()) as u32
                })
                .build_with_hasher(Default::default()),
            #[cfg(feature = "metrics")]
            metrics: Default::default(),
        }
    }
}

impl<S> PrecompileCache<S>
where
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn get(&self, input: &[u8], spec: S) -> Option<CacheEntry<S>> {
        self.cache.get(input).filter(|entry| entry.spec == spec)
    }

    fn insert(&self, input: Bytes, value: CacheEntry<S>) -> usize {
        self.cache.insert(input, value);
        self.cache.entry_count() as usize
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
pub struct CachedPrecompileProvider<T, S>
where
    T: EvmTypesHost,
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    inner: evm2::Precompiles<T>,
    cache_map: PrecompileCacheMap<S>,
    spec_id: S,
    policy: PrecompileCachePolicy,
    #[cfg_attr(not(feature = "metrics"), allow(dead_code))]
    metrics_enabled: bool,
}

impl<T, S> fmt::Debug for CachedPrecompileProvider<T, S>
where
    T: EvmTypesHost,
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachedPrecompileProvider").finish_non_exhaustive()
    }
}

impl<T, S> CachedPrecompileProvider<T, S>
where
    T: EvmTypesHost,
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    /// Creates a new cached precompile provider.
    ///
    /// Metrics are labelled by address and shared with the cache. Disable them for prewarming
    /// so speculative work does not count towards execution cache statistics.
    pub const fn new(
        inner: evm2::Precompiles<T>,
        cache_map: PrecompileCacheMap<S>,
        spec_id: S,
        policy: PrecompileCachePolicy,
        metrics_enabled: bool,
    ) -> Self {
        Self { inner, cache_map, spec_id, policy, metrics_enabled }
    }
}

impl<T, S> PrecompileProvider<T> for CachedPrecompileProvider<T, S>
where
    T: EvmTypesHost,
    S: Eq + Hash + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    fn addresses(&self) -> Vec<Address> {
        self.inner.addresses()
    }

    fn precompile_ids(&self) -> Vec<(Address, evm2::precompiles::PrecompileId)> {
        self.inner.precompile_ids()
    }

    fn move_precompiles(
        &mut self,
        moves: &[(Address, Address)],
    ) -> Result<(), evm2::precompiles::MovePrecompileError> {
        self.inner.move_precompiles(moves)?;
        if moves.iter().any(|(source, dest)| source != dest) {
            if let PrecompileCachePolicy::Addresses(addresses) = &mut self.policy {
                let mut sources = AddressSet::default();
                let moved = moves
                    .iter()
                    .filter(|(source, dest)| source != dest && sources.insert(*source))
                    .map(|(source, dest)| (*source, *dest, addresses.contains(source)))
                    .collect::<Vec<_>>();
                let addresses = Arc::make_mut(addresses);
                for (source, _, _) in &moved {
                    addresses.remove(source);
                }
                for (_, dest, cacheable) in moved {
                    if cacheable {
                        addresses.insert(dest);
                    } else {
                        addresses.remove(&dest);
                    }
                }
            }
            // The shared cache still belongs to the unmodified table used by other EVMs.
            // Relocated entries need a private cache because address alone no longer identifies
            // the same precompile implementation.
            self.cache_map = PrecompileCacheMap::default();
        }
        Ok(())
    }

    fn contains(&self, address: &Address) -> bool {
        self.inner.contains(address)
    }

    fn execute(
        &mut self,
        evm: &mut Evm<'_, T>,
        message: &Message<T>,
        gas: &mut GasTracker,
    ) -> Option<Result<PrecompileOutput, PrecompileError>> {
        let address = message.code_address;
        if !self.policy.allows(&address) {
            return self.inner.execute(evm, message, gas);
        }
        let cache = self.cache_map.cache_for_address(address);
        #[cfg(feature = "metrics")]
        let metrics = self.metrics_enabled.then(|| {
            cache.metrics.get_or_init(|| CachedPrecompileMetrics::new_with_address(address))
        });

        let cacheable_input = message.input.len() <= MAX_PRECOMPILE_CACHE_INPUT_SIZE;
        if cacheable_input &&
            let Some(entry) = cache.get(message.input.as_ref(), self.spec_id.clone())
        {
            return Some(match gas.spend(entry.regular_gas_used).map_err(PrecompileError::from) {
                Ok(()) => {
                    #[cfg(feature = "metrics")]
                    if let Some(metrics) = metrics {
                        metrics.precompile_cache_hits.increment(1);
                    }
                    Ok(entry.to_precompile_result())
                }
                Err(err) => {
                    #[cfg(feature = "metrics")]
                    if let Some(metrics) = metrics {
                        metrics.precompile_errors.increment(1);
                    }
                    Err(err)
                }
            })
        }

        let before = GasSnapshot::new(gas);
        let result = self.inner.execute(evm, message, gas)?;
        let after = GasSnapshot::new(gas);

        match &result {
            Ok(output) if cacheable_input => {
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
                    let _size = cache.insert(
                        Bytes::copy_from_slice(message.input.as_ref()),
                        CacheEntry {
                            output: output.clone(),
                            regular_gas_used,
                            spec: self.spec_id.clone(),
                        },
                    );
                    #[cfg(feature = "metrics")]
                    if let Some(metrics) = metrics {
                        metrics.precompile_cache_size.set(_size as f64);
                        metrics.precompile_cache_misses.increment(1);
                    }
                } else {
                    error!(
                        target: "evm::precompile_cache",
                        %address,
                        "cacheable precompile returned regular gas, skipping cache insertion"
                    );
                }
            }
            Ok(_) => {}
            Err(_) =>
            {
                #[cfg(feature = "metrics")]
                if let Some(metrics) = metrics {
                    metrics.precompile_errors.increment(1);
                }
            }
        }

        Some(result)
    }
}

/// Precompiles whose output and gas usage depend only on input and specification.
///
/// Eligible precompiles must not read execution context or state, mutate state, or emit logs.
/// Custom factories are uncached unless they explicitly select a policy.
#[derive(Debug, Clone, Default)]
pub enum PrecompileCachePolicy {
    /// No precompiles are eligible for caching.
    #[default]
    Disabled,
    /// Every installed precompile is pure and eligible for caching.
    All,
    /// Only these precompile implementations are eligible for caching.
    Addresses(Arc<AddressSet>),
}

impl PrecompileCachePolicy {
    fn allows(&self, address: &Address) -> bool {
        match self {
            Self::Disabled => false,
            Self::All => true,
            Self::Addresses(addresses) => addresses.contains(address),
        }
    }
}

#[derive(Debug)]
struct GasSnapshot {
    spent: u64,
    reservoir: u64,
    state_gas_spent: i64,
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

#[cfg(feature = "metrics")]
impl CachedPrecompileMetrics {
    /// Registers cache metrics labelled by the precompile address.
    pub fn new_with_address(address: Address) -> Self {
        Self::new_with_labels(&[("address", format!("0x{address:02x}"))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, B256, U256};
    use evm2::{
        env::BlockEnv,
        evm::{precompile::NoPrecompiles, InMemoryDB},
        interpreter::{Message, MessageKind},
        registry::TxRegistry,
        BaseEvmTypes, SpecId,
    };

    #[test]
    fn cached_precompile_metadata_matches_inner() {
        let precompiles = evm2::Precompiles::<BaseEvmTypes>::base(SpecId::OSAKA);
        let expected = precompiles.precompile_ids();
        assert!(!expected.is_empty());
        let cached = CachedPrecompileProvider::new(
            precompiles,
            PrecompileCacheMap::default(),
            SpecId::OSAKA,
            PrecompileCachePolicy::All,
            false,
        );
        assert_eq!(cached.precompile_ids(), expected);
    }

    #[test]
    fn moves_preserve_custom_precompiles_with_borrowed_database() {
        let spec = SpecId::OSAKA;
        let identity = Address::with_last_byte(4);
        let custom = Address::with_last_byte(0x40);
        let moved = Address::with_last_byte(0x41);
        let mut precompiles = evm2::Precompiles::<BaseEvmTypes>::base(spec);
        let custom_entry = precompiles.as_map_mut().remove(identity).unwrap().with_address(custom);
        precompiles.as_map_mut().insert(custom_entry);
        let provider = CachedPrecompileProvider::new(
            precompiles,
            PrecompileCacheMap::default(),
            spec,
            PrecompileCachePolicy::All,
            false,
        );
        let mut database = InMemoryDB::default();
        let mut evm = Evm::<BaseEvmTypes>::new(
            spec,
            BlockEnv::<BaseEvmTypes>::default(),
            TxRegistry::new(),
            &mut database,
            provider,
        );
        let standard = Address::with_last_byte(2);
        crate::Evm::move_precompiles(&mut evm, [(standard, moved)]).unwrap();
        assert!(evm.precompiles().contains(&custom));
        assert!(!evm.precompiles().contains(&identity));
        assert!(!evm.precompiles().contains(&standard));
        crate::Evm::move_precompiles(&mut evm, [(custom, identity)]).unwrap();
        assert!(!evm.precompiles().contains(&custom));
        assert!(evm.precompiles().contains(&identity));
        assert!(evm.precompiles().contains(&moved));
        let before = evm.precompiles().precompile_ids();
        assert!(crate::Evm::move_precompiles(&mut evm, [(custom, standard)]).is_err());
        assert_eq!(evm.precompiles().precompile_ids(), before);
    }

    #[test]
    fn moves_isolate_cached_outputs_without_disabling_caching() {
        let spec = SpecId::OSAKA;
        let identity = Address::with_last_byte(4);
        let destination = Address::with_last_byte(2);
        let input = Bytes::from_static(b"input");
        let shared = PrecompileCacheMap::default();
        shared.cache_for_address(destination).insert(
            input.clone(),
            CacheEntry {
                output: PrecompileOutput::new(Bytes::from_static(b"old implementation")),
                regular_gas_used: 60,
                spec,
            },
        );
        let mut provider = CachedPrecompileProvider::new(
            evm2::Precompiles::<BaseEvmTypes>::base(spec),
            shared.clone(),
            spec,
            PrecompileCachePolicy::Addresses(Arc::new(AddressSet::from_iter([identity]))),
            false,
        );
        provider.move_precompiles(&[(identity, destination)]).unwrap();
        let mut evm = Evm::<BaseEvmTypes>::new(
            spec,
            BlockEnv::<BaseEvmTypes>::default(),
            TxRegistry::new(),
            InMemoryDB::default(),
            NoPrecompiles::default(),
        );
        let message = Message::<BaseEvmTypes> {
            kind: MessageKind::Call,
            gas_limit: 30_000,
            destination,
            code_address: destination,
            input: input.clone(),
            ..Default::default()
        };
        for _ in 0..2 {
            let output = provider
                .execute(&mut evm, &message, &mut GasTracker::new(30_000))
                .unwrap()
                .unwrap();
            assert_eq!(output.bytes(), input.as_ref());
        }
        assert_eq!(
            provider
                .cache_map
                .cache_for_address(destination)
                .get(&input, spec)
                .unwrap()
                .output
                .bytes(),
            input.as_ref()
        );
        assert_eq!(
            shared.cache_for_address(destination).get(&input, spec).unwrap().output.bytes(),
            b"old implementation"
        );
    }

    #[test]
    fn caches_successful_precompile_output() {
        let cache_map = PrecompileCacheMap::default();
        let mut provider = CachedPrecompileProvider::new(
            evm2::Precompiles::base(SpecId::OSAKA),
            cache_map.clone(),
            SpecId::OSAKA,
            PrecompileCachePolicy::All,
            false,
        );
        let mut evm = Evm::<BaseEvmTypes>::new(
            SpecId::OSAKA,
            BlockEnv::<BaseEvmTypes>::default(),
            TxRegistry::new(),
            InMemoryDB::default(),
            NoPrecompiles::default(),
        );
        let address = Address::with_last_byte(4);
        let message = Message::<BaseEvmTypes> {
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
    #[test]
    fn identity_cache_obeys_input_size_boundary() {
        let cache = PrecompileCacheMap::default();
        let mut provider = CachedPrecompileProvider::new(
            evm2::Precompiles::base(SpecId::OSAKA),
            cache.clone(),
            SpecId::OSAKA,
            PrecompileCachePolicy::All,
            false,
        );
        let mut evm = Evm::<BaseEvmTypes>::new(
            SpecId::OSAKA,
            BlockEnv::<BaseEvmTypes>::default(),
            TxRegistry::new(),
            InMemoryDB::default(),
            NoPrecompiles::default(),
        );
        let address = Address::with_last_byte(4);
        for len in [2048, 2049] {
            let message = Message::<BaseEvmTypes> {
                kind: MessageKind::Call,
                gas_limit: 30_000,
                destination: address,
                code_address: address,
                input: Bytes::from(vec![1; len]),
                ..Default::default()
            };
            let output = provider
                .execute(&mut evm, &message, &mut GasTracker::new(30_000))
                .unwrap()
                .unwrap();
            assert_eq!(output.bytes(), message.input.as_ref());
            assert_eq!(
                cache
                    .cache_for_address(address)
                    .get(message.input.as_ref(), SpecId::OSAKA)
                    .is_some(),
                len == 2048
            );
        }
    }

    #[test]
    fn caller_dependent_precompiles_are_not_cached() {
        let spec = SpecId::OSAKA;
        let custom = Address::with_last_byte(0x40);
        let identity = Address::with_last_byte(4);
        for policy in [
            PrecompileCachePolicy::Disabled,
            PrecompileCachePolicy::Addresses(Arc::new(AddressSet::from_iter([identity]))),
        ] {
            let cache = PrecompileCacheMap::default();
            let mut precompiles = evm2::Precompiles::<BaseEvmTypes>::base(spec);
            precompiles.as_map_mut().insert(evm2::precompiles::Precompile::new(
                custom,
                evm2::precompiles::PrecompileId::custom("caller"),
                |_, message, gas| {
                    gas.spend(10)?;
                    Ok(PrecompileOutput::new(Bytes::copy_from_slice(message.caller.as_slice())))
                },
            ));
            let mut provider =
                CachedPrecompileProvider::new(precompiles, cache.clone(), spec, policy, false);
            // Move the stateful implementation onto a formerly cacheable address.
            provider.move_precompiles(&[(identity, custom), (custom, identity)]).unwrap();
            let mut evm = Evm::<BaseEvmTypes>::new(
                spec,
                BlockEnv::<BaseEvmTypes>::default(),
                TxRegistry::new(),
                InMemoryDB::default(),
                NoPrecompiles::default(),
            );
            for caller in [Address::repeat_byte(0xaa), Address::repeat_byte(0xbb)] {
                let message = Message::<BaseEvmTypes> {
                    caller,
                    destination: identity,
                    code_address: identity,
                    gas_limit: 30_000,
                    ..Default::default()
                };
                let output = provider
                    .execute(&mut evm, &message, &mut GasTracker::new(30_000))
                    .unwrap()
                    .unwrap();
                assert_eq!(output.bytes(), caller.as_slice());
            }
            assert!(!cache.0.contains_key(&identity));
            assert!(!provider.cache_map.0.contains_key(&identity));
        }
    }
}
