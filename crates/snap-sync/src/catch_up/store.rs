//! Applies verified block access lists to the downloaded state, in strict block order.
//!
//! A list commits with the progress it advances, so recorded progress never runs ahead of the
//! state it describes, and a block whose list is still missing leaves every later one pending.

use crate::{
    common::SnapRecord, AccountCoverage, BalStateUpdate, DownloadedAccount, SnapAccountStore,
    SnapAttemptStore, SnapBytecodeStore, SnapStorageStore, SnapSyncError, SnapWrite,
    StorageProgress,
};
use alloy_eip7928::AccountChanges;
use alloy_eips::BlockNumHash;
use alloy_primitives::{keccak256, B256, KECCAK256_EMPTY};
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::Account;
use reth_storage_api::{
    BlockHashReader, DBProvider, MetadataProvider, MetadataWriter, SnapAttemptId, StateWriter,
};
use serde::{Deserialize, Serialize};

/// Persistence for the block access lists an attempt applies to its downloaded state.
///
/// Blanket-implemented over the node's writers, so the state a list changes, the code it deploys
/// and the progress it advances join the caller's transaction and commit together or not at all.
pub trait SnapCatchUpStore {
    /// Returns the progress recorded for the attempt `write` belongs to, if any.
    fn catch_up_progress(&self, write: SnapWrite)
        -> Result<Option<CatchUpProgress>, SnapSyncError>;

    /// Returns what the downloaded state holds for `hashed_address` under `coverage`.
    fn downloaded_account(
        &self,
        coverage: AccountCoverage,
        hashed_address: B256,
    ) -> Result<DownloadedAccount, SnapSyncError>
    where
        Self: DBProvider;

    /// Returns the state update implied by `bal`, reading downloaded accounts from this provider.
    ///
    /// The list must already be verified against its header. Accounts outside `coverage` remain
    /// unresolved, except for slots of contracts `storage` holds ahead of their range.
    fn block_access_list_update(
        &self,
        coverage: AccountCoverage,
        storage: StorageProgress,
        bal: &[AccountChanges],
    ) -> Result<BalStateUpdate, SnapSyncError>
    where
        Self: DBProvider;

    /// Applies `bal` to the downloaded state and records `block` as the last one applied.
    ///
    /// The list must be authenticated against `block`'s header commitment, and `block` must be
    /// canonical and the child of the last applied one.
    fn commit_block_access_list(
        &self,
        write: SnapWrite,
        block: BlockNumHash,
        parent: B256,
        bal: &[AccountChanges],
    ) -> Result<CatchUpProgress, SnapSyncError>
    where
        Self: BlockHashReader + MetadataWriter + StateWriter + DBProvider<Tx: DbTxMut>;
}

/// How far past the pivot the downloaded state has been carried.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CatchUpProgress {
    // Last block whose list is applied, the pivot until one is.
    applied: BlockNumHash,
}

impl CatchUpProgress {
    /// Nothing applied past `pivot`.
    pub const fn at_pivot(pivot: BlockNumHash) -> Self {
        Self { applied: pivot }
    }

    /// Last block whose list is applied.
    pub const fn applied(&self) -> BlockNumHash {
        self.applied
    }

    /// Number of the block whose list comes next.
    pub const fn next(&self) -> u64 {
        self.applied.number + 1
    }

    // The progress after applying `block`, which must be the child of the applied one.
    fn advance(&self, block: BlockNumHash, parent: B256) -> Result<Self, SnapSyncError> {
        if block.number != self.next() {
            return Err(SnapSyncError::OutOfOrderBlock { expected: self.next(), got: block.number })
        }
        if parent != self.applied.hash {
            return Err(SnapSyncError::ForkedBlock { expected: self.applied.hash, got: parent })
        }
        Ok(Self { applied: block })
    }

    /// Persists this progress as `attempt`'s record.
    pub(crate) fn write(
        &self,
        provider: &impl MetadataWriter,
        attempt: SnapAttemptId,
    ) -> Result<(), SnapSyncError> {
        StoredCatchUpProgress::new(attempt, *self).write(provider)
    }
}

// The progress record as persisted, tied to the attempt that recorded it.
#[derive(Serialize, Deserialize)]
struct StoredCatchUpProgress {
    // Encoding version, checked before the rest is decoded.
    version: u32,
    // Attempt the progress belongs to.
    attempt: SnapAttemptId,
    // Number of the last block whose list is applied.
    block: u64,
    // Hash of that block, which the next one builds on.
    hash: B256,
}

impl SnapRecord for StoredCatchUpProgress {
    const KEY: &'static str = "snap_catch_up_progress";
    const VERSION: u32 = 1;
}

impl StoredCatchUpProgress {
    // `progress` for `attempt` at this build's version.
    const fn new(attempt: SnapAttemptId, progress: CatchUpProgress) -> Self {
        Self {
            version: Self::VERSION,
            attempt,
            block: progress.applied.number,
            hash: progress.applied.hash,
        }
    }

    const fn progress(&self) -> CatchUpProgress {
        CatchUpProgress::at_pivot(BlockNumHash::new(self.block, self.hash))
    }
}

impl<T: MetadataProvider> SnapCatchUpStore for T {
    // A record left by another attempt reads as no progress.
    fn catch_up_progress(
        &self,
        write: SnapWrite,
    ) -> Result<Option<CatchUpProgress>, SnapSyncError> {
        self.authorize_snap_write(write)?;
        let Some(stored) = StoredCatchUpProgress::read(self)? else { return Ok(None) };
        Ok((stored.attempt == write.attempt()).then(|| stored.progress()))
    }

    fn downloaded_account(
        &self,
        coverage: AccountCoverage,
        hashed_address: B256,
    ) -> Result<DownloadedAccount, SnapSyncError>
    where
        Self: DBProvider,
    {
        // Ranges cover the key space in order, so anything from the cursor on is still pending.
        if coverage.next().is_some_and(|next| hashed_address >= next) {
            return Ok(DownloadedAccount::Unknown)
        }
        Ok(self
            .tx_ref()
            .get::<tables::HashedAccounts>(hashed_address)?
            .map_or(DownloadedAccount::Absent, DownloadedAccount::Present))
    }

    fn block_access_list_update(
        &self,
        coverage: AccountCoverage,
        storage: StorageProgress,
        bal: &[AccountChanges],
    ) -> Result<BalStateUpdate, SnapSyncError>
    where
        Self: DBProvider,
    {
        let mut update = BalStateUpdate::default();
        for account_changes in bal {
            let account_info = account_changes.account_info();
            // Read-only entries record accesses, not changes.
            if !account_info.changes_state_root(account_changes) {
                continue
            }

            let hashed_address = keccak256(account_changes.address());
            let mut account = match self.downloaded_account(coverage, hashed_address)? {
                // Its range is downloaded against a later root, which includes this change. Slots
                // persisted ahead of it may predate the change, so they follow the list.
                DownloadedAccount::Unknown => {
                    if storage.has_slots(hashed_address) && account_changes.has_storage_changes() {
                        update.insert_storage(hashed_address, account_changes);
                    }
                    update.unresolved.push(hashed_address);
                    continue
                }
                DownloadedAccount::Absent => Account::default(),
                DownloadedAccount::Present(account) => account,
            };

            account.apply_bal_info(account_info);
            // Stored accounts represent empty code with no code hash.
            account.bytecode_hash = account.bytecode_hash.filter(|hash| *hash != KECCAK256_EMPTY);
            // Execution removes accounts a block leaves empty, see EIP-161.
            update.state.accounts.insert(hashed_address, (!account.is_empty()).then_some(account));
            if account_changes.has_storage_changes() {
                update.insert_storage(hashed_address, account_changes);
            }
            if let Some((code_hash, code)) = account_info
                .code_hash
                .zip(account_changes.code_post_state().filter(|code| !code.is_empty()))
            {
                update.bytecodes.insert(code_hash, code.clone());
            }
        }
        Ok(update)
    }

    // Every check runs before the first write, so a refused list changes nothing.
    fn commit_block_access_list(
        &self,
        write: SnapWrite,
        block: BlockNumHash,
        parent: B256,
        bal: &[AccountChanges],
    ) -> Result<CatchUpProgress, SnapSyncError>
    where
        Self: BlockHashReader + MetadataWriter + StateWriter + DBProvider<Tx: DbTxMut>,
    {
        // Downloaded ranges are proved against the pivot, so it must still be canonical too.
        let attempt = self.authorize_canonical_snap_write(write)?;
        // The chain may have reorged since the list was requested.
        if self.block_hash(block.number)? != Some(block.hash) {
            return Err(SnapSyncError::NonCanonicalBlock { block: block.number, hash: block.hash })
        }
        let advanced = self
            .catch_up_progress(write)?
            .ok_or(SnapSyncError::NoCatchUpProgress)?
            .advance(block, parent)?;
        // Pending ranges are proved against the pivot, so they hold no change past it.
        if block.number > attempt.pivot().number {
            return Err(SnapSyncError::BlockPastPivot {
                pivot: attempt.pivot().number,
                block: block.number,
            })
        }
        let coverage = self.account_coverage(write)?.ok_or(SnapSyncError::NoCoverage)?;
        let storage = match coverage.next() {
            Some(origin) => self.storage_progress(write, origin)?,
            None => StorageProgress::START,
        };
        let update = self.block_access_list_update(coverage, storage, bal)?;

        // Entries left unresolved are downloaded whole against the pivot, which already includes
        // this block, so nothing here has to remember them.
        let (state, bytecodes, _unresolved) = update.into_parts();
        for (hashed_address, account) in &state.accounts {
            // Execution clears the storage of an account it removes, which the list leaves to
            // whoever applies it.
            if account.is_none() {
                self.remove::<tables::HashedStorages>(*hashed_address..=*hashed_address)?;
            }
        }
        self.commit_bytecodes(write, bytecodes.into_iter().collect())?;
        self.write_hashed_state(&state.into_sorted())?;
        advanced.write(self, write.attempt())?;
        Ok(advanced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{
            account, hashed_factory, key, state_root, storage_root_of, BalChain, SnapStateSnapshot,
        },
        SnapAccountStore, SnapGeneration, StorageChunk,
    };
    use alloy_eip7928::{BalanceChange, BlockAccessIndex, CodeChange, SlotChanges, StorageChange};
    use alloy_primitives::{bytes, keccak256, map::B256Map, Address, Bytes, U256};
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::{insert_headers, MockNodeTypesWithDB},
        DatabaseProviderFactory, ProviderFactory,
    };
    use reth_trie_common::{HashedStorage, TrieAccount};

    type Factory = ProviderFactory<MockNodeTypesWithDB>;

    // The account the fixture lists change, holding one storage slot.
    const CHANGED: Address = Address::repeat_byte(0xaa);
    const SLOT: U256 = U256::from_limbs([1, 0, 0, 0]);

    fn hashed_slot() -> B256 {
        keccak256(B256::from(SLOT))
    }

    fn code() -> Bytes {
        bytes!("6001")
    }

    // The downloaded trie: filler accounts a partial range can stop inside, and the account the
    // lists change, holding one slot and nothing but a balance, so a list can empty it.
    fn accounts() -> Vec<(B256, TrieAccount)> {
        let mut changed = account(0);
        changed.storage_root = storage_root_of(&[(hashed_slot(), U256::from(7))]);
        let mut accounts: Vec<_> = (1..=3).map(|nonce| (key(nonce), account(nonce))).collect();
        accounts.push((keccak256(CHANGED), changed));
        accounts.sort_by_key(|(hashed_address, _)| *hashed_address);
        accounts
    }

    // Position of the changed account in the trie.
    fn changed_index(accounts: &[(B256, TrieAccount)]) -> usize {
        accounts
            .iter()
            .position(|(hashed_address, _)| *hashed_address == keccak256(CHANGED))
            .unwrap()
    }

    // An attempt started at block 1, with `served` of the trie's accounts downloaded and its
    // block access list progress recorded, then moved to pivot 2.
    fn started(accounts: &[(B256, TrieAccount)], served: usize) -> (Factory, SnapWrite) {
        let factory = hashed_factory();
        insert_headers(&factory, &chain().headers);
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(1, state_root(accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        // A partial range needs a proof placing the accounts it leaves out.
        let targets = (served < accounts.len()).then(|| accounts[served - 1].0);
        let range =
            crate::test_utils::verified_range(accounts, 0..served, B256::ZERO, targets.as_slice());
        let storages = B256Map::from_iter([(
            keccak256(CHANGED),
            HashedStorage::from_iter([(hashed_slot(), U256::from(7))]),
        )]);
        provider
            .commit_account_range(
                write,
                &range,
                if served == accounts.len() { storages } else { B256Map::default() },
                Vec::new(),
            )
            .unwrap();
        let write =
            provider.advance_snap_pivot(write, generation(2, state_root(accounts))).unwrap();
        provider.commit().unwrap();
        (factory, write)
    }

    // Canonical blocks 0 through 3, pivoted at block 1. The lists the tests apply are their own, as
    // the store does not authenticate them.
    fn chain() -> BalChain {
        BalChain::new(1, [Vec::new(), Vec::new()])
    }

    // Generation anchored to block `number` of the fixture chain.
    fn generation(number: u64, state_root: B256) -> SnapGeneration {
        SnapGeneration::new(chain().block(number as usize - 1), state_root)
    }

    // Block `number` of the fixture chain, with the hash of its parent.
    fn block(number: u64) -> (BlockNumHash, B256) {
        let chain = chain();
        (chain.block(number as usize - 1), chain.block(number as usize - 2).hash)
    }

    // The fixture chain with its last block replaced by one changing a balance.
    fn replacement() -> BalChain {
        BalChain::new(1, [Vec::new(), credit(1)])
    }

    // The attempt carried through block 2 and anchored to block 3, which a reorg then replaced,
    // leaving the applied block canonical. Returns the orphaned pivot.
    fn orphaned_pivot(accounts: &[(B256, TrieAccount)]) -> (Factory, SnapWrite, BlockNumHash) {
        let (factory, write) = started(accounts, accounts.len());
        let provider = factory.database_provider_rw().unwrap();
        let (applied, parent) = block(2);
        provider.commit_block_access_list(write, applied, parent, &[]).unwrap();
        let write =
            provider.advance_snap_pivot(write, generation(3, state_root(accounts))).unwrap();
        provider.commit().unwrap();
        let replacement = replacement();
        assert_eq!(replacement.block(1), applied);
        replacement.replace_tip(&factory);
        (factory, write, block(3).0)
    }

    fn index(value: u64) -> BlockAccessIndex {
        BlockAccessIndex::new(value)
    }

    // A list crediting `CHANGED` with `balance`.
    fn credit(balance: u64) -> Vec<AccountChanges> {
        vec![AccountChanges::new(CHANGED)
            .with_balance_change(BalanceChange::new(index(1), U256::from(balance)))]
    }

    fn stored(provider: &impl DBProvider, hashed_address: B256) -> Option<Account> {
        provider.tx_ref().get::<tables::HashedAccounts>(hashed_address).unwrap()
    }

    #[test]
    fn progress_starts_at_the_first_pivot_and_resumes_where_it_left_off() {
        let accounts = accounts();
        let (factory, write) = started(&accounts, accounts.len());
        let provider = factory.database_provider_rw().unwrap();
        let (block, parent) = block(2);

        // The pivot moved to block 2, but ranges committed at block 1 still need its list.
        assert_eq!(
            provider.catch_up_progress(write).unwrap().unwrap().applied(),
            generation(1, B256::ZERO).target()
        );

        let progress =
            provider.commit_block_access_list(write, block, parent, &credit(10)).unwrap();
        assert_eq!(progress.applied(), block);
        assert_eq!(progress.next(), 3);
        // Restarting the attempt re-applies nothing.
        assert_eq!(provider.catch_up_progress(write).unwrap(), Some(progress));
    }

    #[test]
    fn a_list_commits_with_the_block_it_carries_the_state_to() {
        let accounts = accounts();
        let (factory, write) = started(&accounts, accounts.len());
        let (block, parent) = block(2);
        let changes = vec![AccountChanges::new(CHANGED)
            .with_balance_change(BalanceChange::new(index(1), U256::from(10)))
            .with_code_change(CodeChange::new(index(1), code()))
            .with_storage_change(SlotChanges::new(
                SLOT,
                vec![StorageChange::new(index(1), U256::from(9))],
            ))];

        let provider = factory.database_provider_rw().unwrap();
        provider.commit_block_access_list(write, block, parent, &changes).unwrap();
        provider.commit().unwrap();

        let provider = factory.database_provider_ro().unwrap();
        let applied = stored(&provider, keccak256(CHANGED)).unwrap();
        assert_eq!(applied.balance, U256::from(10));
        assert_eq!(applied.bytecode_hash, Some(keccak256(code())));
        // The code the list deploys is stored with the account referencing it.
        assert!(provider.tx_ref().get::<tables::Bytecodes>(keccak256(code())).unwrap().is_some());
        assert_eq!(
            crate::test_utils::stored_slots(&provider, keccak256(CHANGED)),
            [(hashed_slot(), U256::from(9))]
        );
        assert_eq!(provider.catch_up_progress(write).unwrap().unwrap().applied(), block);
    }

    #[test]
    fn an_interrupted_application_leaves_nothing_behind() {
        let accounts = accounts();
        let (factory, write) = started(&accounts, accounts.len());
        let (block, parent) = block(2);

        let provider = factory.database_provider_rw().unwrap();
        provider.commit_block_access_list(write, block, parent, &credit(10)).unwrap();
        drop(provider);

        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(stored(&provider, keccak256(CHANGED)).unwrap().balance, U256::from(1));
        assert_eq!(provider.catch_up_progress(write).unwrap().unwrap().applied().number, 1);
    }

    #[test]
    fn a_block_already_applied_is_refused() {
        let accounts = accounts();
        let (factory, write) = started(&accounts, accounts.len());
        let provider = factory.database_provider_rw().unwrap();
        let (block, parent) = block(2);
        provider.commit_block_access_list(write, block, parent, &credit(10)).unwrap();

        let duplicate = provider.commit_block_access_list(write, block, parent, &credit(20));

        assert!(matches!(duplicate, Err(SnapSyncError::OutOfOrderBlock { expected: 3, got: 2 })));
        assert_eq!(stored(&provider, keccak256(CHANGED)).unwrap().balance, U256::from(10));
    }

    #[test]
    fn a_block_the_applied_state_has_not_reached_is_refused() {
        let accounts = accounts();
        let (factory, write) = started(&accounts, accounts.len());
        let provider = factory.database_provider_rw().unwrap();
        let (block, parent) = block(3);

        let gap = provider.commit_block_access_list(write, block, parent, &credit(10));

        assert!(matches!(gap, Err(SnapSyncError::OutOfOrderBlock { expected: 2, got: 3 })));
        assert_eq!(stored(&provider, keccak256(CHANGED)).unwrap().balance, U256::from(1));
    }

    #[test]
    fn a_block_the_canonical_chain_no_longer_holds_is_refused() {
        let accounts = accounts();
        let (factory, write) = started(&accounts, accounts.len());
        let provider = factory.database_provider_rw().unwrap();
        let (_, parent) = block(2);
        let orphaned = BlockNumHash::new(2, B256::repeat_byte(0xee));

        let refused = provider.commit_block_access_list(write, orphaned, parent, &credit(10));

        assert!(matches!(refused, Err(SnapSyncError::NonCanonicalBlock { block: 2, .. })));
        assert_eq!(stored(&provider, keccak256(CHANGED)).unwrap().balance, U256::from(1));
    }

    #[test]
    fn a_list_proved_against_an_orphaned_pivot_is_refused() {
        let accounts = accounts();
        let (factory, write, orphaned) = orphaned_pivot(&accounts);
        let provider = factory.database_provider_rw().unwrap();
        let (applied, _) = block(2);
        // The replacement block continues the applied one, so only the pivot check refuses it.
        let (block, parent) = (replacement().block(2), applied.hash);
        let before = SnapStateSnapshot::read(&provider);

        let refused = provider.commit_block_access_list(write, block, parent, &credit(10));

        assert!(matches!(
            refused,
            Err(SnapSyncError::NonCanonicalBlock { block: 3, hash }) if hash == orphaned.hash
        ));
        assert_eq!(provider.catch_up_progress(write).unwrap().unwrap().applied(), applied);
        assert_eq!(SnapStateSnapshot::read(&provider), before);
        provider.commit().unwrap();
        assert_eq!(SnapStateSnapshot::read(&factory.database_provider_ro().unwrap()), before);
    }

    #[test]
    fn an_orphaned_pivot_cannot_be_replaced() {
        let accounts = accounts();
        let (factory, write, orphaned) = orphaned_pivot(&accounts);
        let provider = factory.database_provider_rw().unwrap();
        let before = SnapStateSnapshot::read(&provider);
        let replacement = SnapGeneration::new(replacement().block(2), state_root(&accounts));

        let refused = provider.advance_snap_pivot(write, replacement);

        // Re-anchoring would leave the ranges downloaded against the orphan looking canonical.
        assert!(matches!(
            refused,
            Err(SnapSyncError::NonCanonicalBlock { block: 3, hash }) if hash == orphaned.hash
        ));
        assert_eq!(provider.authorize_snap_write(write).unwrap().pivot(), orphaned);
        assert_eq!(SnapStateSnapshot::read(&provider), before);
        provider.commit().unwrap();
        assert_eq!(SnapStateSnapshot::read(&factory.database_provider_ro().unwrap()), before);
    }

    #[test]
    fn a_block_building_on_another_chain_is_refused() {
        let accounts = accounts();
        let (factory, write) = started(&accounts, accounts.len());
        let provider = factory.database_provider_rw().unwrap();
        let (block, _) = block(2);

        let forked =
            provider.commit_block_access_list(write, block, B256::repeat_byte(0xee), &credit(10));

        assert!(matches!(forked, Err(SnapSyncError::ForkedBlock { .. })));
        assert_eq!(provider.catch_up_progress(write).unwrap().unwrap().applied().number, 1);
    }

    #[test]
    fn a_block_that_changes_nothing_still_carries_the_state_past_it() {
        let accounts = accounts();
        let (factory, write) = started(&accounts, accounts.len());
        let provider = factory.database_provider_rw().unwrap();
        let (block, parent) = block(2);
        // A list a peer holds but that touches no state, as against one it does not hold.
        let read_only = vec![AccountChanges::new(CHANGED).with_storage_read(SLOT)];

        let progress = provider.commit_block_access_list(write, block, parent, &read_only).unwrap();

        assert_eq!(progress.applied(), block);
        assert_eq!(stored(&provider, keccak256(CHANGED)).unwrap().balance, U256::from(1));
    }

    #[test]
    fn an_account_the_list_empties_loses_its_storage() {
        let accounts = accounts();
        let (factory, write) = started(&accounts, accounts.len());
        let provider = factory.database_provider_rw().unwrap();
        let (block, parent) = block(2);

        provider.commit_block_access_list(write, block, parent, &credit(0)).unwrap();

        assert_eq!(stored(&provider, keccak256(CHANGED)), None);
        assert!(crate::test_utils::stored_slots(&provider, keccak256(CHANGED)).is_empty());
    }

    #[test]
    fn a_block_past_the_pivot_is_refused() {
        let accounts = accounts();
        let (factory, write) = started(&accounts, accounts.len());
        let provider = factory.database_provider_rw().unwrap();
        let (block, parent) = block(2);
        provider.commit_block_access_list(write, block, parent, &credit(10)).unwrap();
        let (block, parent) = self::block(3);

        let refused = provider.commit_block_access_list(write, block, parent, &credit(20));

        assert!(matches!(refused, Err(SnapSyncError::BlockPastPivot { pivot: 2, block: 3 })));
        assert_eq!(stored(&provider, keccak256(CHANGED)).unwrap().balance, U256::from(10));
    }

    #[test]
    fn an_account_outside_the_coverage_is_left_to_its_range() {
        let accounts = accounts();
        let index = changed_index(&accounts);
        let (factory, write) = started(&accounts, index);
        let provider = factory.database_provider_rw().unwrap();
        let (block, parent) = block(2);

        let progress =
            provider.commit_block_access_list(write, block, parent, &credit(10)).unwrap();

        // The range still to download authenticates against a root that already holds the change.
        assert_eq!(stored(&provider, keccak256(CHANGED)), None);
        assert_eq!(progress.applied(), block);
    }

    #[test]
    fn slots_persisted_ahead_of_their_range_follow_the_lists() {
        let factory = hashed_factory();
        insert_headers(&factory, &chain().headers);
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(1, state_root(&accounts()))).unwrap();
        provider.start_account_coverage(write).unwrap();
        let (changed, stored_slot) = (keccak256(CHANGED), (hashed_slot(), U256::from(7)));
        let chunk = StorageChunk::new(
            changed,
            storage_root_of(&[stored_slot]),
            B256::ZERO,
            vec![stored_slot],
            None,
        );
        provider.commit_storage_chunk(write, B256::ZERO, chunk).unwrap();
        let mut moved = accounts();
        let position = changed_index(&moved);
        moved[position].1.storage_root = storage_root_of(&[(hashed_slot(), U256::from(9))]);
        let write = provider.advance_snap_pivot(write, generation(2, state_root(&moved))).unwrap();
        let (block, parent) = block(2);
        let changes = vec![AccountChanges::new(CHANGED).with_storage_change(SlotChanges::new(
            SLOT,
            vec![StorageChange::new(index(1), U256::from(9))],
        ))];

        provider.commit_block_access_list(write, block, parent, &changes).unwrap();

        // Only the slots follow: the account itself comes whole with its range.
        assert_eq!(stored(&provider, changed), None);
        assert_eq!(
            crate::test_utils::stored_slots(&provider, changed),
            [(hashed_slot(), U256::from(9))]
        );
        let range = crate::test_utils::verified_range(&moved, 0..moved.len(), B256::ZERO, &[]);
        let coverage =
            provider.commit_account_range(write, &range, B256Map::default(), Vec::new()).unwrap();
        assert!(coverage.is_complete());
    }

    #[test]
    fn skipped_storage_from_an_abandoned_attempt_is_not_reused() {
        let factory = hashed_factory();
        insert_headers(&factory, &chain().headers);
        let provider = factory.database_provider_rw().unwrap();
        let (changed, later) = (keccak256(CHANGED), B256::repeat_byte(0xff));
        let stale = (B256::ZERO, U256::from(7));
        let mut contract = account(1);
        contract.storage_root = storage_root_of(&[stale]);
        let mut accounts = vec![(changed, contract), (later, contract)];
        let write = provider.start_snap_attempt(generation(1, state_root(&accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        provider
            .commit_storage_chunk(
                write,
                B256::ZERO,
                StorageChunk::new(changed, contract.storage_root, B256::ZERO, vec![stale], None),
            )
            .unwrap();

        accounts[0].1.storage_root = reth_trie_common::EMPTY_ROOT_HASH;
        let write = provider.start_snap_attempt(generation(1, state_root(&accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        provider
            .commit_storage_chunk(
                write,
                B256::ZERO,
                StorageChunk::new(later, contract.storage_root, B256::ZERO, vec![stale], None),
            )
            .unwrap();

        let new_slot = (hashed_slot(), U256::from(9));
        accounts[0].1.storage_root = storage_root_of(&[new_slot]);
        let write =
            provider.advance_snap_pivot(write, generation(2, state_root(&accounts))).unwrap();
        let (block, parent) = block(2);
        let changes = vec![AccountChanges::new(CHANGED).with_storage_change(SlotChanges::new(
            SLOT,
            vec![StorageChange::new(index(1), new_slot.1)],
        ))];
        provider.commit_block_access_list(write, block, parent, &changes).unwrap();

        assert_eq!(crate::test_utils::stored_slots(&provider, changed), [new_slot]);
        let range =
            crate::test_utils::verified_range(&accounts, 0..accounts.len(), B256::ZERO, &[]);
        assert!(provider
            .commit_account_range(write, &range, B256Map::default(), Vec::new())
            .unwrap()
            .is_complete());
    }

    #[test]
    fn a_list_from_a_replaced_attempt_changes_nothing() {
        let accounts = accounts();
        let (factory, write) = started(&accounts, accounts.len());
        let provider = factory.database_provider_rw().unwrap();
        provider.advance_snap_pivot(write, generation(3, B256::repeat_byte(0xcc))).unwrap();
        let (block, parent) = block(2);

        let refused = provider.commit_block_access_list(write, block, parent, &credit(10));

        assert!(matches!(refused, Err(SnapSyncError::StaleWrite { .. })));
        assert!(matches!(provider.catch_up_progress(write), Err(SnapSyncError::StaleWrite { .. })));
    }

    #[test]
    fn a_record_this_build_cannot_read_is_reported() {
        let accounts = accounts();
        let (factory, write) = started(&accounts, accounts.len());
        let provider = factory.database_provider_rw().unwrap();
        provider.write_metadata(StoredCatchUpProgress::KEY, br#"{"version":0}"#.to_vec()).unwrap();

        assert!(matches!(
            provider.catch_up_progress(write),
            Err(SnapSyncError::UnsupportedRecord {
                key: StoredCatchUpProgress::KEY,
                version: Some(0)
            })
        ));
    }
}
