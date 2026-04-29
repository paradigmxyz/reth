//! Snap protocol state provider backed by a chain-aware provider.
//!
//! Serves recent historical state for request roots by applying a revert overlay
//! on top of the current MDBX hashed state. This keeps the served state fully
//! persisted and deterministic.

use alloy_consensus::{BlockHeader, EMPTY_ROOT_HASH};
use alloy_primitives::{Bytes, B256};
use reth_db_api::transaction::DbTx;
use reth_eth_wire_types::{
    snap::{AccountData, SlimAccount, StorageData},
    BlockAccessLists,
};
use reth_network_p2p::snap::server::SnapStateProvider;
use reth_stages_types::StageId;
use reth_storage_api::{
    BalProvider, BlockNumReader, ChangeSetReader, DBProvider, DatabaseProviderFactory,
    HeaderProvider, StageCheckpointReader, StorageChangeSetReader,
};
use reth_trie::{
    hashed_cursor::{HashedCursor, HashedCursorFactory, HashedPostStateCursorFactory},
    HashedPostStateSorted,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseHashedPostState};
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
        + BlockNumReader,
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
        + BlockNumReader,
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

        let mut accounts = Vec::new();
        let mut total_bytes: u64 = 0;

        if let Ok(Some((hash, account))) = cursor.seek(starting_hash) &&
            hash < limit_hash
        {
            let body =
                Bytes::from(alloy_rlp::encode(SlimAccount::from_account(account, EMPTY_ROOT_HASH)));
            total_bytes += body.len() as u64 + 32;
            accounts.push(AccountData { hash, body });
        }

        while accounts.len() < MAX_ACCOUNTS_SERVE && total_bytes < response_bytes {
            match cursor.next() {
                Ok(Some((hash, account))) if hash < limit_hash => {
                    let body = Bytes::from(alloy_rlp::encode(SlimAccount::from_account(
                        account,
                        EMPTY_ROOT_HASH,
                    )));
                    total_bytes += body.len() as u64 + 32;
                    accounts.push(AccountData { hash, body });
                }
                _ => break,
            }
        }

        (accounts, Vec::new())
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

        let mut all_slots = Vec::new();
        let mut total_bytes: u64 = 0;

        for (i, account_hash) in account_hashes.iter().enumerate() {
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

            while total_bytes < response_bytes {
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

            all_slots.push(slots);
            if total_bytes >= response_bytes {
                break;
            }
        }

        (all_slots, Vec::new())
    }

    fn bytecodes(&self, hashes: Vec<B256>, response_bytes: u64) -> Vec<Bytes> {
        let Ok(provider) = self.provider.database_provider_ro() else {
            return Vec::new();
        };
        let tx = provider.tx_ref();

        let mut codes = Vec::new();
        let mut total_bytes: u64 = 0;

        for hash in hashes {
            if let Ok(Some(bytecode)) = tx.get::<reth_db_api::tables::Bytecodes>(hash) {
                let raw = Bytes::from(bytecode.bytes().to_vec());
                total_bytes += raw.len() as u64;
                codes.push(raw);
                if total_bytes >= response_bytes {
                    break;
                }
            } else {
                codes.push(Bytes::new());
            }
        }

        codes
    }

    fn block_access_lists(&self, block_hashes: Vec<B256>, response_bytes: u64) -> BlockAccessLists {
        let results = match self.provider.bal_store().get_by_hashes(&block_hashes) {
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
}
