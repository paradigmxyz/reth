//! File-based persistence for NetworkStateCache.
//!
//! Allows saving/loading the cache state to disk so it survives restarts.

use crate::{
    network_cache::{CachedEntry, NetworkStateCache},
    policy::{AccountData, CachePolicy},
};
use alloy_primitives::{Address, Bytes, B256, U256};
use std::{fs, io, path::Path};
use tracing::info;

/// Serializable representation of the cache state (without policy).
///
/// Policies are intentionally excluded: they are behavior, not state, and are
/// supplied separately when reconstructing a cache (see [`CacheState::into_cache`]).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheState {
    pub accounts: Vec<(Address, CachedEntry<AccountData>)>,
    pub storage: Vec<((Address, B256), CachedEntry<U256>)>,
    pub codes: Vec<(B256, CachedEntry<Bytes>)>,
    pub current_block: u64,
}

impl CacheState {
    /// Capture a serializable snapshot of a live cache's contents.
    pub fn from_cache(cache: &NetworkStateCache) -> Self {
        Self {
            accounts: cache.accounts().iter().map(|(k, v)| (*k, v.clone())).collect(),
            storage: cache.storage().iter().map(|(k, v)| (*k, v.clone())).collect(),
            codes: cache.codes().iter().map(|(k, v)| (*k, v.clone())).collect(),
            current_block: cache.current_block(),
        }
    }

    /// Reconstruct a live cache from this state, attaching the given policies.
    ///
    /// The restored cache has no undo history, so a reorg deeper than the state
    /// captured here triggers a cold reset (see [`NetworkStateCache::restore`]).
    pub fn into_cache(
        self,
        account_policy: Box<dyn CachePolicy>,
        storage_policy: Box<dyn CachePolicy>,
    ) -> NetworkStateCache {
        NetworkStateCache::restore(
            self.accounts.into_iter().collect(),
            self.storage.into_iter().collect(),
            self.codes.into_iter().collect(),
            self.current_block,
            account_policy,
            storage_policy,
        )
    }
}

/// Save the current cache state to a file using bincode.
pub fn save_to_file(cache: &NetworkStateCache, path: &Path) -> io::Result<()> {
    let state = CacheState::from_cache(cache);

    let encoded = bincode::serialize(&state)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    // Write atomically via temp file
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, &encoded)?;
    fs::rename(&tmp_path, path)?;

    info!(
        target: "partial_stateless::persistence",
        path = %path.display(),
        bytes = encoded.len(),
        accounts = cache.accounts().len(),
        storage = cache.storage().len(),
        "Cache saved to file"
    );

    Ok(())
}

/// Load cache state from a file and reconstruct with the given policies.
///
/// The policies are provided separately because they are not serialized.
pub fn load_from_file(
    path: &Path,
    account_policy: Box<dyn CachePolicy>,
    storage_policy: Box<dyn CachePolicy>,
) -> io::Result<NetworkStateCache> {
    let data = fs::read(path)?;
    let state: CacheState = bincode::deserialize(&data)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    info!(
        target: "partial_stateless::persistence",
        path = %path.display(),
        block = state.current_block,
        accounts = state.accounts.len(),
        storage = state.storage.len(),
        codes = state.codes.len(),
        "Cache loaded from file"
    );

    Ok(state.into_cache(account_policy, storage_policy))
}
