//! Snap protocol state provider backed by a chain-aware provider.
//!
//! Serves recent historical state for request roots by applying a revert overlay
//! on top of the current MDBX hashed state. This keeps the served state fully
//! persisted and deterministic.

use alloy_consensus::{BlockHeader, EMPTY_ROOT_HASH};
use alloy_primitives::{map::B256Map, Bytes, B256};
use reth_db_api::transaction::DbTx;
use reth_eth_wire_types::{
    snap::{AccountData, StorageData},
    BlockAccessLists,
};
use reth_network_p2p::snap::server::SnapStateProvider;
use reth_provider::LatestStateProviderRef;
use reth_stages_types::StageId;
use reth_storage_api::{
    BalProvider, BlockHashReader, BlockNumReader, BytecodeReader, ChangeSetReader, DBProvider,
    DatabaseProviderFactory, HeaderProvider, StageCheckpointReader, StorageChangeSetReader,
    StorageSettingsCache,
};
use reth_trie::{
    hashed_cursor::{HashedCursor, HashedCursorFactory, HashedPostStateCursorFactory},
    prefix_set::PrefixSetMut,
    proof::{Proof, StorageProof},
    HashedPostStateSorted, HashedStorageSorted, MultiProofTargets, Nibbles,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseHashedPostState, DatabaseTrieCursorFactory,
};
use std::sync::atomic::{AtomicU64, Ordering};

/// Maximum accounts to return per snap request.
const MAX_ACCOUNTS_SERVE: usize = 4096;

/// Default maximum number of recent blocks to scan when resolving a root hash.
const DEFAULT_MAX_SERVING_LOOKBACK: u64 = 128;

/// Maximum number of recent blocks to scan when resolving a root hash.
static MAX_SERVING_LOOKBACK: AtomicU64 = AtomicU64::new(DEFAULT_MAX_SERVING_LOOKBACK);

/// Snap state provider that wraps a chain-aware provider and serves historical
/// state via a revert overlay.
///
/// The provider `P` must implement [`BlockNumReader`] and [`HeaderProvider`]
/// directly so request roots can be resolved against the canonical in-memory
/// tip, not just static-file-persisted blocks. In practice, pass a
/// `BlockchainProvider`.
pub struct ProviderSnapState<P> {
    provider: P,
}

impl<P> ProviderSnapState<P> {
    /// Create a new snap state provider.
    pub const fn new(provider: P) -> Self {
        Self { provider }
    }
}

impl<P> core::fmt::Debug for ProviderSnapState<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ProviderSnapState").finish()
    }
}

/// Guard that restores the previous snap serving lookback when dropped.
#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug)]
pub struct MaxServingLookbackGuard {
    previous: u64,
}

#[cfg(any(test, feature = "test-utils"))]
impl Drop for MaxServingLookbackGuard {
    fn drop(&mut self) {
        MAX_SERVING_LOOKBACK.store(self.previous, Ordering::Relaxed);
    }
}

/// Overrides the serving lookback for tests.
#[cfg(any(test, feature = "test-utils"))]
pub fn set_max_serving_lookback_for_tests(lookback: u64) -> MaxServingLookbackGuard {
    let previous = MAX_SERVING_LOOKBACK.swap(lookback, Ordering::Relaxed);
    MaxServingLookbackGuard { previous }
}

impl<P> ProviderSnapState<P>
where
    P: HeaderProvider + BlockNumReader,
{
    /// Scan recent headers for one whose state root matches `root_hash` and
    /// return its block number.
    fn resolve_serving_block(&self, root_hash: B256) -> Option<u64> {
        let tip = self.provider.best_block_number().ok()?;
        let start = tip.saturating_sub(MAX_SERVING_LOOKBACK.load(Ordering::Relaxed));
        for num in (start..=tip).rev() {
            if let Ok(Some(header)) = self.provider.header_by_number(num) {
                if header.state_root() == root_hash {
                    return Some(num);
                }
            }
        }
        None
    }
}

impl<P> ProviderSnapState<P>
where
    P: DatabaseProviderFactory + HeaderProvider + BlockNumReader,
    P::Provider: DBProvider
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockHashReader
        + BlockNumReader
        + StorageSettingsCache,
    <P::Provider as DBProvider>::Tx: DbTx,
{
    fn database_provider_with_reverts(
        &self,
        root_hash: B256,
    ) -> Option<(P::Provider, HashedPostStateSorted)> {
        let serving_block = self.resolve_serving_block(root_hash)?;
        let provider = self.provider.database_provider_ro().ok()?;

        let persisted = provider.get_stage_checkpoint(StageId::Execution).ok()??.block_number;
        let revert_state = if persisted > serving_block {
            HashedPostStateSorted::from_reverts(&provider, (serving_block + 1)..=persisted).ok()?
        } else {
            HashedPostStateSorted::default()
        };

        Some((provider, revert_state))
    }
}

impl<P> SnapStateProvider for ProviderSnapState<P>
where
    P: DatabaseProviderFactory
        + HeaderProvider
        + BlockNumReader
        + BalProvider
        + Send
        + Sync
        + 'static,
    P::Provider: DBProvider
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
    <P::Provider as DBProvider>::Tx: DbTx,
{
    fn account_range(
        &self,
        root_hash: B256,
        starting_hash: B256,
        limit_hash: B256,
        response_bytes: u64,
    ) -> (Vec<AccountData>, Vec<Bytes>) {
        let empty = (Vec::new(), Vec::new());

        let Some((provider, revert_state)) = self.database_provider_with_reverts(root_hash) else {
            return empty;
        };

        let cursor_factory = HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(provider.tx_ref()),
            &revert_state,
        );

        let Ok(mut cursor) = cursor_factory.hashed_account_cursor() else { return empty };

        let mut raw_accounts = Vec::new();
        let mut total_bytes: u64 = 0;

        if let Ok(Some((hash, account))) = cursor.seek(starting_hash) {
            let body_len = snap_account_body_len_upper_bound(account);
            total_bytes += body_len as u64 + 32;
            raw_accounts.push((hash, account));

            if hash < limit_hash {
                while raw_accounts.len() < MAX_ACCOUNTS_SERVE && total_bytes < response_bytes {
                    match cursor.next() {
                        Ok(Some((hash, account))) if hash < limit_hash => {
                            let body_len = snap_account_body_len_upper_bound(account);
                            total_bytes += body_len as u64 + 32;
                            raw_accounts.push((hash, account));
                        }
                        _ => break,
                    }
                }
            }
        }

        let Some((storage_roots, proof)) =
            account_range_proof(&provider, &revert_state, starting_hash, &raw_accounts)
        else {
            return empty;
        };

        let accounts = raw_accounts
            .into_iter()
            .map(|(hash, account)| {
                let storage_root = storage_roots.get(&hash).copied().unwrap_or(EMPTY_ROOT_HASH);
                AccountData { hash, account: account.into_trie_account(storage_root) }
            })
            .collect();

        (accounts, proof)
    }

    fn storage_ranges(
        &self,
        root_hash: B256,
        account_hashes: Vec<B256>,
        starting_hash: B256,
        limit_hash: B256,
        response_bytes: u64,
    ) -> (Vec<Vec<StorageData>>, Vec<Bytes>) {
        let empty = (Vec::new(), Vec::new());

        let Some((provider, revert_state)) = self.database_provider_with_reverts(root_hash) else {
            return empty;
        };

        let cursor_factory = HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(provider.tx_ref()),
            &revert_state,
        );

        if !requested_accounts_available(&cursor_factory, &account_hashes).unwrap_or(false) {
            return empty;
        }

        let mut all_slots: Vec<Vec<StorageData>> = Vec::new();
        let mut total_bytes: u64 = 0;
        let mut partial_range = None;

        for (i, account_hash) in account_hashes.iter().enumerate() {
            let prior_slots_returned = all_slots.iter().any(|slots| !slots.is_empty());
            if total_bytes >= response_bytes && prior_slots_returned {
                break;
            }

            let mut slots = Vec::new();
            let start = if i == 0 { starting_hash } else { B256::ZERO };

            let Ok(mut cursor) = cursor_factory.hashed_storage_cursor(*account_hash) else {
                all_slots.push(slots);
                continue;
            };

            if let Ok(Some((key, value))) = cursor.seek(start) &&
                key < limit_hash &&
                !value.is_zero()
            {
                let slot = StorageData::from_value(key, value);
                total_bytes += slot.data.len() as u64 + 32;
                slots.push(slot);
            }

            while total_bytes < response_bytes || (!prior_slots_returned && slots.is_empty()) {
                match cursor.next() {
                    Ok(Some((key, value))) if key < limit_hash => {
                        if value.is_zero() {
                            continue;
                        }
                        let slot = StorageData::from_value(key, value);
                        total_bytes += slot.data.len() as u64 + 32;
                        slots.push(slot);
                    }
                    _ => break,
                }
            }

            if total_bytes >= response_bytes &&
                storage_has_more_slots(&mut cursor, limit_hash).unwrap_or(false)
            {
                partial_range = Some((*account_hash, start, slots.last().map(|slot| slot.hash)));
            }

            all_slots.push(slots);
            if partial_range.is_some() {
                break;
            }
        }

        let proof = partial_range
            .and_then(|(account_hash, start, last_hash)| {
                storage_range_proof(&provider, &revert_state, account_hash, start, last_hash)
            })
            .unwrap_or_default();

        (all_slots, proof)
    }

    fn bytecodes(&self, hashes: Vec<B256>, response_bytes: u64) -> Vec<Bytes> {
        let Ok(provider) = self.provider.database_provider_ro() else {
            return Vec::new();
        };

        let mut out = Vec::new();
        let mut total: u64 = 0;
        let state = LatestStateProviderRef::new(&provider);
        for hash in hashes {
            if total >= response_bytes {
                break;
            }
            if let Ok(Some(code)) = state.bytecode_by_hash(&hash) {
                let bytes = code.original_bytes();
                total += bytes.len() as u64;
                out.push(bytes);
            }
        }
        out
    }

    fn block_access_lists(&self, block_hashes: Vec<B256>, response_bytes: u64) -> BlockAccessLists {
        serve_block_access_lists(&self.provider, block_hashes, response_bytes)
    }
}

fn account_range_proof<Provider>(
    provider: &Provider,
    revert_state: &HashedPostStateSorted,
    starting_hash: B256,
    accounts: &[(B256, reth_primitives_traits::Account)],
) -> Option<(B256Map<B256>, Vec<Bytes>)>
where
    Provider: DBProvider + StorageSettingsCache,
    Provider::Tx: DbTx,
{
    reth_trie_db::with_adapter!(provider, |A| {
        let mut targets = MultiProofTargets::accounts([starting_hash]);
        targets.extend(MultiProofTargets::accounts(accounts.iter().map(|(hash, _)| *hash)));

        let multiproof = Proof::new(
            DatabaseTrieCursorFactory::<_, A>::new(provider.tx_ref()),
            HashedPostStateCursorFactory::new(
                DatabaseHashedCursorFactory::new(provider.tx_ref()),
                revert_state,
            ),
        )
        .with_prefix_sets_mut(revert_state.construct_prefix_sets())
        .multiproof(targets)
        .ok()?;

        let storage_roots =
            multiproof.storages.iter().map(|(hash, proof)| (*hash, proof.root)).collect();
        let proof = multiproof
            .account_subtree
            .into_nodes_sorted()
            .into_iter()
            .map(|(_, node)| node)
            .collect();

        Some((storage_roots, proof))
    })
}

fn requested_accounts_available<CF>(
    cursor_factory: &CF,
    account_hashes: &[B256],
) -> Result<bool, reth_db_api::DatabaseError>
where
    CF: HashedCursorFactory,
{
    let mut cursor = cursor_factory.hashed_account_cursor()?;
    for account_hash in account_hashes {
        if !matches!(cursor.seek(*account_hash)?, Some((hash, _)) if hash == *account_hash) {
            return Ok(false);
        }
        cursor.reset();
    }
    Ok(true)
}

fn snap_account_body_len_upper_bound(account: reth_primitives_traits::Account) -> usize {
    AccountData::account_body_len(account.into_trie_account(B256::ZERO))
}

fn storage_range_proof<Provider>(
    provider: &Provider,
    revert_state: &HashedPostStateSorted,
    account_hash: B256,
    starting_hash: B256,
    last_hash: Option<B256>,
) -> Option<Vec<Bytes>>
where
    Provider: DBProvider + StorageSettingsCache,
    Provider::Tx: DbTx,
{
    reth_trie_db::with_adapter!(provider, |A| {
        let targets = last_hash.map_or_else(
            || alloy_primitives::map::B256Set::from_iter([starting_hash]),
            |last_hash| alloy_primitives::map::B256Set::from_iter([starting_hash, last_hash]),
        );

        let multiproof = StorageProof::new_hashed(
            DatabaseTrieCursorFactory::<_, A>::new(provider.tx_ref()),
            HashedPostStateCursorFactory::new(
                DatabaseHashedCursorFactory::new(provider.tx_ref()),
                revert_state,
            ),
            account_hash,
        )
        .with_prefix_set_mut(storage_prefix_set_mut(revert_state, account_hash))
        .storage_multiproof(targets)
        .ok()?;

        Some(multiproof.subtree.into_nodes_sorted().into_iter().map(|(_, node)| node).collect())
    })
}

fn storage_prefix_set_mut(
    revert_state: &HashedPostStateSorted,
    account_hash: B256,
) -> PrefixSetMut {
    match revert_state.account_storages().get(&account_hash) {
        Some(storage) => storage_prefix_set_from_sorted(storage),
        None => PrefixSetMut::default(),
    }
}

fn storage_prefix_set_from_sorted(storage: &HashedStorageSorted) -> PrefixSetMut {
    if storage.wiped {
        return PrefixSetMut::all();
    }

    let mut prefix_set = PrefixSetMut::with_capacity(storage.storage_slots.len());
    prefix_set.extend_keys(storage.storage_slots.iter().map(|(slot, _)| Nibbles::unpack(slot)));
    prefix_set
}

fn storage_has_more_slots<C>(
    cursor: &mut C,
    limit_hash: B256,
) -> Result<bool, reth_db_api::DatabaseError>
where
    C: HashedCursor<Value = alloy_primitives::U256>,
{
    while let Some((key, value)) = cursor.next()? {
        if key >= limit_hash {
            return Ok(false);
        }
        if !value.is_zero() {
            return Ok(true);
        }
    }

    Ok(false)
}

fn serve_block_access_lists<P>(
    provider: &P,
    block_hashes: Vec<B256>,
    response_bytes: u64,
) -> BlockAccessLists
where
    P: BalProvider,
{
    let results = match provider.bal_store().get_by_hashes(&block_hashes) {
        Ok(results) => results,
        Err(_) => return BlockAccessLists(Vec::new()),
    };

    let mut total_bytes = 0u64;
    let mut out = Vec::new();
    for bal in results {
        let bal = bal.unwrap_or_else(|| Bytes::from_static(&[alloy_rlp::EMPTY_STRING_CODE]));
        total_bytes += bal.len() as u64;
        out.push(bal);

        if total_bytes >= response_bytes {
            break;
        }
    }

    BlockAccessLists(out)
}
