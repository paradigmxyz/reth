//! Snap protocol state provider backed by a chain-aware provider.
//!
//! Serves historical state at `HEAD - SERVING_OFFSET` by applying a revert
//! overlay on top of the current MDBX hashed state. This guarantees the
//! served state is always fully persisted and deterministic.

use alloy_consensus::{constants::KECCAK_EMPTY, EMPTY_ROOT_HASH};
use alloy_primitives::{Bytes, B256};
use alloy_rlp::Encodable;
use reth_db_api::transaction::DbTx;
use reth_network::{
    snap_requests::SnapStateProvider,
    types::snap::{AccountData, StorageData},
};
use reth_primitives_traits::Account;
use reth_stages::StageId;
use reth_storage_api::{
    BlockNumReader, ChangeSetReader, DBProvider, DatabaseProviderFactory, HeaderProvider,
    StageCheckpointReader, StorageChangeSetReader,
};
use reth_trie::{
    hashed_cursor::{HashedCursor, HashedCursorFactory, HashedPostStateCursorFactory},
    HashedPostStateSorted,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseHashedPostState};

/// Number of blocks behind the canonical tip to serve.
/// HEAD minus this offset is guaranteed to be fully persisted in MDBX.
const SERVING_OFFSET: u64 = 16;

/// Maximum accounts to return per snap request.
const MAX_ACCOUNTS_SERVE: usize = 4096;

/// Snap state provider that wraps a chain-aware provider and serves
/// historical state at `HEAD - SERVING_OFFSET` via a revert overlay.
///
/// The provider `P` must implement [`BlockNumReader`] and [`HeaderProvider`]
/// directly (not just via inner DB providers) so that `current_state_root()`
/// can see the canonical in-memory tip, not just the static-file-persisted
/// blocks. In practice, pass a [`BlockchainProvider`] here.
pub struct ProviderSnapState<P> {
    provider: P,
}

impl<P> ProviderSnapState<P> {
    /// Create a new snap state provider.
    pub const fn new(provider: P) -> Self {
        Self { provider }
    }
}

impl<P> std::fmt::Debug for ProviderSnapState<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderSnapState").finish()
    }
}

/// Encode an [`Account`] as slim-format RLP: `[nonce, balance, storage_root, code_hash]`.
fn encode_slim_account(account: &Account) -> Bytes {
    let code_hash = account.bytecode_hash.unwrap_or(KECCAK_EMPTY);
    let mut payload = Vec::new();
    account.nonce.encode(&mut payload);
    account.balance.encode(&mut payload);
    EMPTY_ROOT_HASH.encode(&mut payload);
    code_hash.encode(&mut payload);

    let mut buf = Vec::new();
    alloy_rlp::Header { list: true, payload_length: payload.len() }.encode(&mut buf);
    buf.extend_from_slice(&payload);
    Bytes::from(buf)
}

/// Maximum number of recent blocks to scan when resolving a root hash.
const MAX_SERVING_LOOKBACK: u64 = 128;

impl<P> ProviderSnapState<P>
where
    P: HeaderProvider + BlockNumReader,
{
    /// Scan the last [`MAX_SERVING_LOOKBACK`] headers for one whose state root
    /// matches `root_hash` and return its block number.
    fn resolve_serving_block(&self, root_hash: B256) -> Option<u64> {
        let tip = self.provider.best_block_number().ok()?;
        let start = tip.saturating_sub(MAX_SERVING_LOOKBACK);
        for num in (start..=tip).rev() {
            if let Ok(Some(header)) = self.provider.header_by_number(num) {
                use alloy_consensus::BlockHeader;
                if header.state_root() == root_hash {
                    return Some(num);
                }
            }
        }
        None
    }
}

impl<P> SnapStateProvider for ProviderSnapState<P>
where
    P: DatabaseProviderFactory + HeaderProvider + BlockNumReader + Send + Sync + 'static,
    P::Provider: DBProvider
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader,
    <P::Provider as DBProvider>::Tx: DbTx,
{
    fn current_state_root(&self) -> B256 {
        let Ok(tip) = self.provider.best_block_number() else {
            return B256::ZERO;
        };
        let serving_block = tip.saturating_sub(SERVING_OFFSET);

        let Ok(Some(header)) = self.provider.header_by_number(serving_block) else {
            return B256::ZERO;
        };

        use alloy_consensus::BlockHeader;
        header.state_root()
    }

    fn account_range(
        &self,
        root_hash: B256,
        starting_hash: B256,
        limit_hash: B256,
        response_bytes: u64,
    ) -> (Vec<AccountData>, Vec<Bytes>) {
        let empty = (Vec::new(), Vec::new());

        let Some(serving_block) = self.resolve_serving_block(root_hash) else { return empty };

        let Ok(provider) = self.provider.database_provider_ro() else { return empty };

        // The persisted tip is what HashedAccounts currently reflects.
        let persisted = match provider.get_stage_checkpoint(StageId::Execution) {
            Ok(Some(cp)) => cp.block_number,
            _ => return empty,
        };

        // Build the revert overlay: undo changes from (serving_block, persisted]
        // to project HashedAccounts back to serving_block.
        let revert_state = if persisted > serving_block {
            match HashedPostStateSorted::from_reverts(
                &provider,
                (serving_block + 1)..=persisted,
            ) {
                Ok(state) => state,
                Err(_) => return empty,
            }
        } else {
            Default::default()
        };

        let cursor_factory = HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(provider.tx_ref()),
            &revert_state,
        );

        let Ok(mut cursor) = cursor_factory.hashed_account_cursor() else { return empty };

        let mut accounts = Vec::new();
        let mut total_bytes: u64 = 0;

        if let Ok(Some((hash, account))) = cursor.seek(starting_hash)
            && hash < limit_hash
        {
            let body = encode_slim_account(&account);
            total_bytes += body.len() as u64 + 32;
            accounts.push(AccountData { hash, body });
        }

        while accounts.len() < MAX_ACCOUNTS_SERVE && total_bytes < response_bytes {
            match cursor.next() {
                Ok(Some((hash, account))) if hash < limit_hash => {
                    let body = encode_slim_account(&account);
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

        let Some(serving_block) = self.resolve_serving_block(root_hash) else { return empty };

        let Ok(provider) = self.provider.database_provider_ro() else { return empty };

        let persisted = match provider.get_stage_checkpoint(StageId::Execution) {
            Ok(Some(cp)) => cp.block_number,
            _ => return empty,
        };

        let revert_state = if persisted > serving_block {
            match HashedPostStateSorted::from_reverts(
                &provider,
                (serving_block + 1)..=persisted,
            ) {
                Ok(state) => state,
                Err(_) => return empty,
            }
        } else {
            Default::default()
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

            if let Ok(Some((key, value))) = cursor.seek(start)
                && key < limit_hash
                && !value.is_zero()
            {
                let mut data_buf = Vec::new();
                value.encode(&mut data_buf);
                let data = Bytes::from(data_buf);
                total_bytes += data.len() as u64 + 32;
                slots.push(StorageData { hash: key, data });
            }

            while total_bytes < response_bytes {
                match cursor.next() {
                    Ok(Some((key, value))) if key < limit_hash => {
                        if value.is_zero() {
                            continue;
                        }
                        let mut data_buf = Vec::new();
                        value.encode(&mut data_buf);
                        let data = Bytes::from(data_buf);
                        total_bytes += data.len() as u64 + 32;
                        slots.push(StorageData { hash: key, data });
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
}
