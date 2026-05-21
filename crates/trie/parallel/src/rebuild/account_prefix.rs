use alloy_primitives::B256;
use reth_primitives_traits::Account;
use reth_trie::{
    hashed_cursor::{HashedCursor, HashedCursorFactory},
    updates::TrieUpdates,
    HashBuilder, Nibbles,
};
use std::{sync::Arc, time::Duration};

use super::{
    storage_prefix_bound, storage_prefix_end, AccountStoragePresence,
    ParallelRebuildStateRootError, StoragePresenceCursor, StorageRootPrefetchConfig,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AccountPrefixPlan {
    pub(super) items: Vec<AccountPrefixPlanItem>,
    pub(super) stats: AccountPrefixPlannerStats,
    pub(super) complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AccountPrefixPlanItem {
    Range(AccountPrefixRange),
    Barrier(AccountPrefixBarrier),
    Single(AccountPrefixSingle),
}

impl AccountPrefixPlanItem {
    pub(super) const fn start(&self) -> B256 {
        match self {
            Self::Range(range) => range.start,
            Self::Barrier(barrier) => barrier.hashed_address,
            Self::Single(single) => single.hashed_address,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct AccountPrefixPlannerStats {
    pub(super) planned_ranges: usize,
    pub(super) planned_items: usize,
    pub(super) accounts: usize,
    pub(super) storage_presence_checks: usize,
    pub(super) storage_empty_hits: usize,
    pub(super) storage_present_hits: usize,
    pub(super) storage_skipped_keys: usize,
    pub(super) storage_count_queries: usize,
    pub(super) storage_counted_slots: usize,
    pub(super) max_account_storage_slots: usize,
    pub(super) large_storage_accounts: usize,
    pub(super) large_storage_barriers: usize,
    pub(super) total_weight: u64,
    pub(super) max_range_weight: u64,
    pub(super) planning_duration: Duration,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AccountPrefixWeight {
    weight: u64,
    accounts: usize,
    storage_slots: usize,
    large_storage_accounts: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AccountPrefixRange {
    pub(super) prefix: Nibbles,
    pub(super) start: B256,
    pub(super) end: Option<B256>,
    pub(super) weight: u64,
    pub(super) storage_hints: Option<Arc<[AccountPrefixStorageHint]>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AccountPrefixBarrier {
    pub(super) hashed_address: B256,
    pub(super) account: Account,
    pub(super) storage_slots: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AccountPrefixSingle {
    pub(super) hashed_address: B256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AccountPrefixStorageHint {
    pub(super) hashed_address: B256,
    pub(super) slots: Option<usize>,
}

#[derive(Debug)]
pub(super) struct AccountPrefixResult {
    pub(super) prefix: Nibbles,
    pub(super) root_hash: B256,
    pub(super) last_hashed_address: B256,
    pub(super) walked_accounts: usize,
    pub(super) walked_storage_slots: usize,
    pub(super) updates: TrieUpdates,
    pub(super) stats: super::StorageRootPrefetchStats,
}

pub(super) struct AccountPrefixBuildState {
    pub(super) hash_builder: HashBuilder,
    pub(super) updates: TrieUpdates,
    pub(super) stats: super::StorageRootPrefetchStats,
    pub(super) account_rlp: Vec<u8>,
    pub(super) walked_accounts: usize,
    pub(super) walked_storage_slots: usize,
}

const MAX_ACCOUNT_PREFIX_DEPTH: usize = 4;
const ACCOUNT_PREFIX_ACCOUNT_WEIGHT: u64 = 1;

pub(super) fn account_prefix_bound(prefix: &Nibbles) -> B256 {
    storage_prefix_bound(prefix)
}

fn account_prefix_end(prefix: &Nibbles) -> Option<B256> {
    storage_prefix_end(prefix)
}

pub(super) fn plan_account_prefixes_window<H>(
    hashed_cursor_factory: &H,
    max_depth: usize,
    config: StorageRootPrefetchConfig,
    last_account_key: Option<B256>,
    item_limit: usize,
) -> Result<AccountPrefixPlan, ParallelRebuildStateRootError>
where
    H: HashedCursorFactory,
{
    let started = std::time::Instant::now();
    if max_depth == 0 || max_depth > MAX_ACCOUNT_PREFIX_DEPTH {
        return Err(ParallelRebuildStateRootError::InvalidAccountPrefixDepth(max_depth))
    }

    let total_prefixes = 16usize.pow(max_depth as u32);
    let start_index =
        account_prefix_window_start_index(last_account_key, max_depth, total_prefixes);
    let mut levels = (1..=max_depth)
        .map(|depth| vec![AccountPrefixWeight::default(); 16usize.pow(depth as u32)])
        .collect::<Vec<_>>();
    let mut account_cursor = hashed_cursor_factory.hashed_account_cursor()?;
    let storage_start = if start_index < total_prefixes {
        account_prefix_bound(&account_prefix_from_index(start_index, max_depth))
    } else {
        B256::repeat_byte(0xff)
    };
    let mut storage_presence = hashed_cursor_factory
        .hashed_storage_key_cursor()?
        .map(|cursor| StoragePresenceCursor::from_start(cursor, storage_start))
        .transpose()?;
    let collect_storage_hints = storage_presence.is_some();
    let min_large_slots = config.storage_prefix_min_large_slots.max(1);
    let mut stats = AccountPrefixPlannerStats::default();
    let mut items = Vec::new();
    let item_limit = item_limit.max(1);
    let weight_limit = if config.progress_threshold == u64::MAX {
        u64::MAX
    } else {
        config.progress_threshold.max(1)
    };
    let mut planned_weight = 0u64;
    let mut complete = true;

    let mut prefix_index = start_index;
    while prefix_index < total_prefixes {
        let prefix = account_prefix_from_index(prefix_index, max_depth);
        let range = AccountPrefixRange {
            start: account_prefix_bound(&prefix),
            end: account_prefix_end(&prefix),
            prefix,
            weight: 0,
            storage_hints: None,
        };
        let mut barriers = Vec::new();
        let mut storage_hints = Vec::new();
        let mut entry = account_cursor.seek(range.start)?;
        while let Some((hashed_address, account)) = entry {
            if range.end.is_some_and(|end| hashed_address >= end) {
                break
            }

            let path = Nibbles::unpack(hashed_address);
            if !path.starts_with(&range.prefix) {
                break
            }

            stats.accounts += 1;
            let (presence, skipped_keys) = if let Some(storage_presence) = storage_presence.as_mut()
            {
                storage_presence.lookup(hashed_address)?
            } else {
                (AccountStoragePresence::Present { slots: None }, 0)
            };
            stats.storage_presence_checks += 1;
            stats.storage_skipped_keys += skipped_keys;

            let (storage_slots, storage_slots_known, large_storage_account) = match presence {
                AccountStoragePresence::Empty => {
                    stats.storage_empty_hits += 1;
                    (0usize, true, false)
                }
                AccountStoragePresence::Present { slots } => {
                    stats.storage_present_hits += 1;
                    if collect_storage_hints {
                        storage_hints.push(AccountPrefixStorageHint { hashed_address, slots });
                    }
                    if let Some(slots) = slots {
                        stats.storage_count_queries += 1;
                        stats.storage_counted_slots += slots;
                        stats.max_account_storage_slots =
                            stats.max_account_storage_slots.max(slots);
                        (slots, true, slots >= min_large_slots)
                    } else {
                        (1usize, false, false)
                    }
                }
            };

            let storage_weight = if storage_slots_known { storage_slots as u64 } else { 1 };
            let account_weight = ACCOUNT_PREFIX_ACCOUNT_WEIGHT.saturating_add(storage_weight);
            for depth in 1..=max_depth {
                let range = &mut levels[depth - 1][account_prefix_index_from_path(&path, depth)];
                range.weight = range.weight.saturating_add(account_weight);
                range.accounts += 1;
                range.storage_slots = range.storage_slots.saturating_add(storage_slots);
                if large_storage_account {
                    range.large_storage_accounts += 1;
                }
            }
            stats.large_storage_accounts += usize::from(large_storage_account);
            stats.total_weight = stats.total_weight.saturating_add(account_weight);
            if account_prefix_should_barrier_storage(storage_slots, config) {
                barriers.push(AccountPrefixBarrier { hashed_address, account, storage_slots });
                stats.large_storage_barriers += 1;
            }

            entry = account_cursor.next()?;
        }

        let mut prefix_items = Vec::new();
        let prefix_weight = levels[max_depth - 1][prefix_index];
        if !barriers.is_empty() && prefix_weight.accounts == barriers.len() {
            barriers.sort_unstable_by_key(|barrier| barrier.hashed_address);
            prefix_items.extend(barriers.into_iter().map(AccountPrefixPlanItem::Barrier));
        } else {
            emit_account_prefix_plan_item(
                range.prefix,
                &levels,
                max_depth,
                &barriers,
                &mut prefix_items,
            );
        }
        let storage_hints = collect_storage_hints.then_some(storage_hints.as_slice());
        for item in prefix_items
            .into_iter()
            .map(|item| account_prefix_plan_item_with_storage_hints(item, storage_hints))
            .filter(|item| {
                last_account_key.is_none_or(|last_account_key| item.start() > last_account_key)
            })
        {
            if let AccountPrefixPlanItem::Range(range) = &item {
                stats.planned_ranges += 1;
                stats.max_range_weight = stats.max_range_weight.max(range.weight);
            }
            items.push(item);
        }
        planned_weight = planned_weight.saturating_add(prefix_weight.weight);
        prefix_index += 1;

        if !items.is_empty() && (items.len() >= item_limit || planned_weight >= weight_limit) {
            complete = prefix_index == total_prefixes;
            break
        }
    }

    stats.planned_items = items.len();
    stats.planning_duration += started.elapsed();

    Ok(AccountPrefixPlan { items, stats, complete })
}

const fn account_prefix_should_barrier_storage(
    storage_slots: usize,
    config: StorageRootPrefetchConfig,
) -> bool {
    config.progress_threshold != u64::MAX && storage_slots as u64 > config.progress_threshold
}

fn emit_account_prefix_plan_item(
    prefix: Nibbles,
    levels: &[Vec<AccountPrefixWeight>],
    max_depth: usize,
    barriers: &[AccountPrefixBarrier],
    items: &mut Vec<AccountPrefixPlanItem>,
) {
    let barrier_descendants = barriers
        .iter()
        .copied()
        .filter(|barrier| Nibbles::unpack(barrier.hashed_address).starts_with(&prefix))
        .collect::<Vec<_>>();
    if barrier_descendants.is_empty() {
        if prefix.len() == 64 {
            items.push(AccountPrefixPlanItem::Single(AccountPrefixSingle {
                hashed_address: account_prefix_bound(&prefix),
            }));
            return
        }
        let Some(range) = account_prefix_range_from_weighted_prefix(prefix, levels, max_depth)
        else {
            return
        };
        items.push(AccountPrefixPlanItem::Range(range));
        return
    }

    if prefix.len() == 64 {
        let barrier = barrier_descendants[0];
        items.push(AccountPrefixPlanItem::Barrier(barrier));
        return
    }

    for nibble in 0..16 {
        let mut child = prefix;
        child.push(nibble);
        emit_account_prefix_plan_item(child, levels, max_depth, barriers, items);
    }
}

fn account_prefix_plan_item_with_storage_hints(
    item: AccountPrefixPlanItem,
    storage_hints: Option<&[AccountPrefixStorageHint]>,
) -> AccountPrefixPlanItem {
    match item {
        AccountPrefixPlanItem::Range(mut range) => {
            if let Some(storage_hints) = storage_hints {
                let range_hints = storage_hints
                    .iter()
                    .copied()
                    .filter(|hint| account_prefix_range_contains(&range, hint.hashed_address))
                    .collect::<Vec<_>>();
                range.storage_hints = Some(Arc::from(range_hints.into_boxed_slice()));
            }
            AccountPrefixPlanItem::Range(range)
        }
        item => item,
    }
}

pub(super) fn account_prefix_range_contains(
    range: &AccountPrefixRange,
    hashed_address: B256,
) -> bool {
    hashed_address >= range.start && range.end.is_none_or(|end| hashed_address < end)
}

fn account_prefix_range_from_weighted_prefix(
    prefix: Nibbles,
    levels: &[Vec<AccountPrefixWeight>],
    max_depth: usize,
) -> Option<AccountPrefixRange> {
    let weight = if prefix.len() <= max_depth {
        let weight = levels[prefix.len() - 1][account_prefix_index_from_prefix(&prefix)];
        if weight.accounts == 0 {
            return None
        }
        weight
    } else {
        AccountPrefixWeight::default()
    };

    Some(AccountPrefixRange {
        start: account_prefix_bound(&prefix),
        end: account_prefix_end(&prefix),
        prefix,
        weight: weight.weight,
        storage_hints: None,
    })
}

fn account_prefix_index_from_path(path: &Nibbles, depth: usize) -> usize {
    path.iter()
        .take(depth)
        .fold(0usize, |index, nibble| index.saturating_mul(16).saturating_add(usize::from(nibble)))
}

fn account_prefix_window_start_index(
    last_account_key: Option<B256>,
    depth: usize,
    total_prefixes: usize,
) -> usize {
    let Some(last_account_key) = last_account_key else { return 0 };
    let path = Nibbles::unpack(last_account_key);
    let index = account_prefix_index_from_path(&path, depth);
    if path.iter().skip(depth).all(|nibble| nibble == 0xf) {
        index.saturating_add(1).min(total_prefixes)
    } else {
        index
    }
}

pub(super) fn account_prefix_last_key(prefix: &Nibbles) -> B256 {
    account_prefix_end(prefix).and_then(b256_previous).unwrap_or(B256::repeat_byte(0xff))
}

fn b256_previous(key: B256) -> Option<B256> {
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(key.as_slice());
    for index in (0..bytes.len()).rev() {
        if bytes[index] != 0 {
            bytes[index] = bytes[index].saturating_sub(1);
            bytes[index + 1..].fill(0xff);
            return Some(B256::from(bytes))
        }
    }
    None
}

fn account_prefix_from_index(mut index: usize, depth: usize) -> Nibbles {
    let mut nibbles = vec![0u8; depth];
    for nibble in nibbles.iter_mut().rev() {
        *nibble = (index % 16) as u8;
        index /= 16;
    }
    Nibbles::from_nibbles(nibbles)
}

fn account_prefix_index_from_prefix(prefix: &Nibbles) -> usize {
    prefix
        .iter()
        .fold(0usize, |index, nibble| index.saturating_mul(16).saturating_add(usize::from(nibble)))
}
