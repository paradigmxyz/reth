//! Recovers downloaded state from a reorg that orphans the pivot.
//!
//! [EIP-8189](https://eips.ethereum.org/EIPS/eip-8189#synchronization-algorithm) repairs it from
//! the abandoned branch's lists: every field they change is fetched again unless a list of the
//! new branch overwrites it first. The reorg removes that branch's headers from the canonical
//! chain, so the attempt keeps them from the moment it anchors.

use crate::{common::SnapRecord, SnapSyncError};
use alloy_eips::{eip1898::BlockWithParent, BlockNumHash};
use alloy_primitives::{Sealable, B256};
use reth_primitives_traits::{AlloyBlockHeader, SealedHeader};
use reth_storage_api::{
    BlockHashReader, HeaderProvider, MetadataProvider, MetadataWriter, SnapAttempt, SnapAttemptId,
};
use serde::{Deserialize, Serialize};

// Blocks an attempt keeps through its pivot, bounding how deep a recoverable reorg can reach.
const KEPT_BLOCKS: u64 = 64;

/// Where a reorg left an attempt: the last block both branches share and the orphaned blocks
/// after it.
#[derive(Clone, Debug)]
pub struct SnapReorg {
    // Last block both branches share.
    ancestor: BlockNumHash,
    // Orphaned blocks through the pivot, oldest first. Empty while the pivot is canonical.
    orphaned: Vec<KeptBlock>,
}

impl SnapReorg {
    /// Last block both branches share.
    pub const fn ancestor(&self) -> BlockNumHash {
        self.ancestor
    }

    /// Orphaned blocks through the pivot, oldest first.
    pub fn orphaned(&self) -> &[KeptBlock] {
        &self.orphaned
    }
}

/// A block an attempt keeps through its pivot, holding what recovery from a reorg reads of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptBlock {
    // Block number and hash, with its parent's hash.
    block: BlockWithParent,
    // Block access list commitment, absent before their activation.
    commitment: Option<B256>,
}

impl KeptBlock {
    /// Block number and hash, with its parent's hash.
    pub const fn block(&self) -> BlockWithParent {
        self.block
    }

    /// Block access list commitment, absent before their activation.
    pub const fn commitment(&self) -> Option<B256> {
        self.commitment
    }
}

impl<H: AlloyBlockHeader + Sealable> From<&SealedHeader<H>> for KeptBlock {
    fn from(header: &SealedHeader<H>) -> Self {
        Self { block: header.block_with_parent(), commitment: header.block_access_list_hash() }
    }
}

// Blocks an attempt keeps through its pivot.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredAncestry {
    // Encoding version, checked before the rest is decoded.
    version: u32,
    // Attempt the blocks belong to.
    attempt: SnapAttemptId,
    // Kept blocks, oldest first and ending at the pivot.
    blocks: Vec<KeptBlock>,
}

impl SnapRecord for StoredAncestry {
    const KEY: &'static str = "snap_ancestry";
    const VERSION: u32 = 1;
}

impl StoredAncestry {
    // Keeps the canonical blocks of the last `KEPT_BLOCKS` through `pivot` for `attempt`. Keeps
    // none unless they reach the pivot, leaving a reorg unrecoverable.
    pub(crate) fn record<P: HeaderProvider + MetadataWriter>(
        provider: &P,
        attempt: SnapAttemptId,
        pivot: BlockNumHash,
    ) -> Result<(), SnapSyncError> {
        let from = pivot.number.saturating_sub(KEPT_BLOCKS) + 1;
        let headers = provider.sealed_headers_range(from..=pivot.number)?;
        let contiguous = headers.windows(2).all(|pair| pair[1].parent_hash() == pair[0].hash());
        if !contiguous || headers.last().map(SealedHeader::num_hash) != Some(pivot) {
            return Self::clear(provider)
        }
        let blocks = headers.iter().map(KeptBlock::from).collect();
        Self { version: Self::VERSION, attempt, blocks }.write(provider)
    }

    // Finds where the canonical chain diverges from the blocks `attempt` keeps. `None` when they
    // do not reach back to where the branches part.
    pub(crate) fn reorg<P: MetadataProvider + BlockHashReader>(
        provider: &P,
        attempt: &SnapAttempt,
    ) -> Result<Option<SnapReorg>, SnapSyncError> {
        let Some(mut blocks) = Self::read(provider)?
            .filter(|stored| stored.attempt == attempt.id())
            .map(|stored| stored.blocks)
        else {
            return Ok(None)
        };
        // Kept blocks end at the pivot they were recorded for.
        if blocks.last().map(|kept| kept.block.block) != Some(attempt.pivot()) {
            return Ok(None)
        }

        // The highest kept block still canonical is where the branches part.
        let mut split = None;
        for (index, kept) in blocks.iter().enumerate().rev() {
            let block = kept.block.block;
            if provider.block_hash(block.number)? == Some(block.hash) {
                split = Some((block, index + 1));
                break
            }
        }
        let (ancestor, from) = match split {
            Some(split) => split,
            None => {
                // Otherwise they part just below the lowest kept block, or further down.
                let lowest = blocks[0].block;
                let Some(parent) = lowest.block.number.checked_sub(1) else { return Ok(None) };
                if provider.block_hash(parent)? != Some(lowest.parent) {
                    return Ok(None)
                }
                (BlockNumHash::new(parent, lowest.parent), 0)
            }
        };
        Ok(Some(SnapReorg { ancestor, orphaned: blocks.split_off(from) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{account, hashed_factory, state_root, verified_range, BalChain},
        SnapAccountStore, SnapAttemptStore, SnapCatchUpStore, SnapGeneration, SnapWrite,
        StateRepairs,
    };
    use alloy_eip7928::{
        AccountChanges, BalanceChange, BlockAccessIndex, NonceChange, SlotChanges, StorageChange,
    };
    use alloy_eips::eip7928::bal::{Bal, DecodedBal};
    use alloy_primitives::{keccak256, Address, B256, U256};
    use reth_provider::{
        test_utils::{insert_headers, MockNodeTypesWithDB},
        DatabaseProviderFactory, ProviderFactory,
    };
    use reth_storage_api::DBProvider;
    use reth_trie_common::TrieAccount;

    type Factory = ProviderFactory<MockNodeTypesWithDB>;

    // Block both branches share.
    const ANCESTOR: u64 = 2;

    fn credit(address: Address, balance: u64) -> Vec<AccountChanges> {
        vec![AccountChanges::new(address)
            .with_balance_change(BalanceChange::new(BlockAccessIndex::new(1), U256::from(balance)))]
    }

    // Blocks 3 and 4 of the orphaned branch, and 3 through 5 of the one replacing it.
    fn branches() -> (BalChain, BalChain) {
        let address = Address::repeat_byte(0x11);
        (
            BalChain::new(ANCESTOR, [credit(address, 1), credit(address, 2)]),
            BalChain::new(
                ANCESTOR,
                [credit(address, 10), credit(address, 20), credit(address, 30)],
            ),
        )
    }

    // Three accounts, of which the first two are downloaded.
    fn accounts() -> (Vec<Address>, Vec<(B256, TrieAccount)>) {
        let mut addresses: Vec<_> = (1..=3).map(Address::repeat_byte).collect();
        addresses.sort_by_key(|address| keccak256(address));
        let accounts = addresses
            .iter()
            .enumerate()
            .map(|(nonce, address)| (keccak256(address), account(nonce as u64)))
            .collect();
        (addresses, accounts)
    }

    // An attempt anchored at `chain`'s block `number`, with the first two accounts downloaded.
    fn started(chain: &BalChain, number: u64) -> (Factory, SnapWrite) {
        let (_, accounts) = accounts();
        let factory = hashed_factory();
        insert_headers(&factory, &chain.headers);
        let provider = factory.database_provider_rw().unwrap();
        let pivot = chain.headers[number as usize].num_hash();
        let write =
            provider.start_snap_attempt(SnapGeneration::new(pivot, state_root(&accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        let range = verified_range(&accounts, 0..2, B256::ZERO, &[B256::ZERO, accounts[1].0]);
        provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();
        provider.commit().unwrap();
        (factory, write)
    }

    fn reorg(factory: &Factory, write: SnapWrite) -> Option<SnapReorg> {
        factory.database_provider_ro().unwrap().snap_reorg(write).unwrap()
    }

    fn kept(headers: &[SealedHeader]) -> Vec<KeptBlock> {
        headers.iter().map(KeptBlock::from).collect()
    }

    // A verified list carrying `changes`.
    fn list(changes: Vec<AccountChanges>) -> DecodedBal {
        DecodedBal::from_rlp_bytes(alloy_rlp::encode(Bal::from(changes)).into()).unwrap()
    }

    #[test]
    fn a_canonical_pivot_orphans_nothing() {
        let (old, _) = branches();
        let (factory, write) = started(&old, 4);

        let reorg = reorg(&factory, write).unwrap();

        assert_eq!(reorg.ancestor(), old.tip());
        assert!(reorg.orphaned().is_empty());
    }

    #[test]
    fn the_ancestor_is_the_last_block_both_branches_share() {
        let (old, new) = branches();
        let (factory, write) = started(&old, 4);
        new.replace_after(&factory, ANCESTOR);

        let reorg = reorg(&factory, write).unwrap();

        assert_eq!(reorg.ancestor(), old.block(0));
        assert_eq!(reorg.orphaned(), kept(&old.headers[3..]));
    }

    #[test]
    fn the_ancestor_can_sit_just_below_the_kept_headers() {
        let address = Address::repeat_byte(0x11);
        let old = BalChain::new(0, [credit(address, 1), credit(address, 2)]);
        let new = BalChain::new(0, [credit(address, 10)]);
        let (factory, write) = started(&old, 2);
        new.replace_after(&factory, 0);

        let reorg = reorg(&factory, write).unwrap();

        assert_eq!(reorg.ancestor(), old.block(0));
        assert_eq!(reorg.orphaned(), kept(&old.headers[1..]));
    }

    #[test]
    fn a_reorg_below_the_kept_headers_is_unrecoverable() {
        let address = Address::repeat_byte(0x11);
        let depth = KEPT_BLOCKS + 2;
        let old = BalChain::new(1, (0..depth).map(|n| credit(address, n)));
        let new = BalChain::new(1, (0..depth).map(|n| credit(address, n + 100)));
        let (factory, write) = started(&old, 1 + depth);
        new.replace_after(&factory, 1);

        assert!(reorg(&factory, write).is_none());
    }

    #[test]
    fn recovery_rewinds_catch_up_to_the_ancestor_and_refuses_old_writes() {
        let (old, new) = branches();
        let (factory, write) = started(&old, 4);
        new.replace_after(&factory, ANCESTOR);
        let (_, accounts) = accounts();
        let generation = SnapGeneration::new(new.tip(), state_root(&accounts));

        let provider = factory.database_provider_rw().unwrap();
        let recovered =
            provider.commit_reorg_recovery(write, old.block(0), &[], generation).unwrap();
        provider.commit().unwrap();

        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.catch_up_progress(recovered).unwrap().unwrap().applied(), old.block(0));
        assert!(matches!(
            provider.authorize_snap_write(write),
            Err(SnapSyncError::StaleWrite { .. })
        ));
        drop(provider);
        // The kept headers follow the new pivot.
        let reorg = reorg(&factory, recovered).unwrap();
        assert_eq!(reorg.ancestor(), new.tip());
    }

    #[test]
    fn a_new_pivot_reorged_meanwhile_is_refused_without_scheduling() {
        let (old, new) = branches();
        let (factory, write) = started(&old, 4);
        new.replace_after(&factory, ANCESTOR);
        let (addresses, accounts) = accounts();
        // The chain moved again, so the selected pivot is no longer canonical.
        let generation = SnapGeneration::new(old.tip(), state_root(&accounts));
        let lists = [list(credit(addresses[0], 5))];

        let provider = factory.database_provider_rw().unwrap();
        let refused = provider.commit_reorg_recovery(write, old.block(0), &lists, generation);
        assert!(matches!(refused, Err(SnapSyncError::NonCanonicalBlock { block: 4, .. })));
        drop(provider);

        // The refused transaction is dropped, leaving the attempt to recover on the next pass.
        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.snap_repairs(write).unwrap().is_empty());
        assert_eq!(reorg(&factory, write).unwrap().ancestor(), old.block(0));
    }

    #[test]
    fn catch_up_below_the_ancestor_stays_where_it_is() {
        let (old, new) = branches();
        let (factory, write) = started(&old, 1);
        let (_, accounts) = accounts();
        let provider = factory.database_provider_rw().unwrap();
        let write = provider
            .advance_snap_pivot(write, SnapGeneration::new(old.tip(), state_root(&accounts)))
            .unwrap();
        provider.commit().unwrap();
        new.replace_after(&factory, ANCESTOR);

        let provider = factory.database_provider_rw().unwrap();
        let generation = SnapGeneration::new(new.tip(), state_root(&accounts));
        let recovered =
            provider.commit_reorg_recovery(write, old.block(0), &[], generation).unwrap();

        assert_eq!(
            provider.catch_up_progress(recovered).unwrap().unwrap().applied(),
            old.headers[1].num_hash()
        );
    }

    #[test]
    fn orphaned_changes_to_downloaded_accounts_are_scheduled() {
        let (old, new) = branches();
        let (factory, write) = started(&old, 4);
        new.replace_after(&factory, ANCESTOR);
        let (addresses, accounts) = accounts();
        let slot = SlotChanges::new(
            U256::from(7),
            vec![StorageChange::new(BlockAccessIndex::new(1), U256::ONE)],
        );
        let downloaded = credit(addresses[0], 5).remove(0).with_storage_change(slot);
        let lists = [
            list(vec![downloaded.clone()]),
            list(vec![
                // Only reads the account, which changes nothing.
                AccountChanges::new(addresses[1]).with_storage_read(U256::ONE),
                // Past the cursor, so downloaded whole later.
                AccountChanges::new(addresses[2])
                    .with_nonce_change(NonceChange::new(BlockAccessIndex::new(1), 1)),
            ]),
        ];
        let generation = SnapGeneration::new(new.tip(), state_root(&accounts));

        let provider = factory.database_provider_rw().unwrap();
        let recovered =
            provider.commit_reorg_recovery(write, old.block(0), &lists, generation).unwrap();

        let mut expected = StateRepairs::default();
        expected.insert_changes(keccak256(addresses[0]), &downloaded);
        assert_eq!(provider.snap_repairs(recovered).unwrap(), expected);
    }
}
