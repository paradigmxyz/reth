#![allow(missing_docs)]

//! Integration tests for execution witness generation at historical blocks.
//!
//! These tests exercise the full historical witness path used by
//! `debug_executionWitness` for non-tip blocks:
//! `HistoricalStateProviderRef::witness` -> overlay construction from trie changesets
//! (`ChangesetCache` / `compute_block_trie_changesets`) -> `TrieWitness` over
//! `InMemoryTrieCursorFactory`.
//!
//! For each historical block the produced witness must be *complete*: starting from the
//! parent block's state root, every trie node visited while resolving each key touched by
//! the block must be present in the witness. This is exactly the property stateless
//! validators rely on.

use alloy_consensus::{Header, EMPTY_ROOT_HASH};
use alloy_primitives::{keccak256, map::B256Map, Address, Bytes, B256, U256};
use alloy_rlp::Decodable;
use reth_db::tables;
use reth_db_api::{
    models::{AccountBeforeTx, BlockNumberAddress},
    transaction::DbTxMut,
};
use reth_primitives_traits::{Account, StorageEntry};
use reth_provider::{
    test_utils::create_test_provider_factory, DBProvider, HashingWriter,
    HistoricalStateProviderRef, StageCheckpointWriter, StaticFileProviderFactory,
    StaticFileSegment, StaticFileWriter, TrieWriter,
};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::StateProofProvider;
use reth_trie::{
    prefix_set::{PrefixSetMut, TriePrefixSets},
    test_utils::state_root_prehashed,
    BranchNode, ExecutionWitnessMode, HashedPostState, HashedStorage, Nibbles, RlpNode, StateRoot,
    TrieAccount, TrieInput, TrieNode,
};
use reth_trie_db::{ChangesetCache, DatabaseStateRoot};
use std::collections::BTreeMap;

type DbStateRoot<'a, TX, A> = StateRoot<
    reth_trie_db::DatabaseTrieCursorFactory<&'a TX, A>,
    reth_trie_db::DatabaseHashedCursorFactory<&'a TX>,
>;

/// Deterministic xorshift* RNG so failures are reproducible from the seed alone.
struct Rng(u64);

impl Rng {
    const fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    const fn gen_range(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    fn b256(&mut self) -> B256 {
        let mut out = [0u8; 32];
        for chunk in out.chunks_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        B256::new(out)
    }

    fn address(&mut self) -> Address {
        Address::from_word(self.b256())
    }

    fn account(&mut self) -> Account {
        Account {
            nonce: self.next_u64(),
            balance: U256::from(self.next_u64()),
            bytecode_hash: None,
        }
    }
}

/// Plain (unhashed) view of the full state at some block.
type PlainState = BTreeMap<Address, (Account, BTreeMap<B256, U256>)>;

/// Per-block state diff in plain keyspace. `None` account means deletion, zero storage
/// value means slot deletion.
#[derive(Default, Clone)]
struct BlockDiff {
    accounts: BTreeMap<Address, Option<Account>>,
    storages: BTreeMap<Address, BTreeMap<B256, U256>>,
}

impl BlockDiff {
    fn touched_addresses(&self) -> impl Iterator<Item = &Address> {
        self.accounts.keys().chain(self.storages.keys())
    }
}

fn apply_diff(state: &mut PlainState, diff: &BlockDiff) {
    // Storage first, accounts second: a selfdestruct pairs an account deletion with
    // zero-writes for every slot, and the deletion must win.
    for (address, slots) in &diff.storages {
        let entry = state.entry(*address).or_insert_with(|| (Account::default(), BTreeMap::new()));
        for (slot, value) in slots {
            if value.is_zero() {
                entry.1.remove(slot);
            } else {
                entry.1.insert(*slot, *value);
            }
        }
    }
    for (address, account) in &diff.accounts {
        match account {
            Some(account) => {
                state.entry(*address).or_insert_with(|| (*account, BTreeMap::new())).0 = *account;
            }
            None => {
                state.remove(address);
            }
        }
    }
}

/// Generates the initial block: account creations, some with storage.
fn gen_initial_diff(rng: &mut Rng, num_accounts: usize) -> BlockDiff {
    let mut diff = BlockDiff::default();
    for i in 0..num_accounts {
        let address = rng.address();
        diff.accounts.insert(address, Some(rng.account()));
        // Roughly a quarter of the accounts are contracts with storage.
        if i % 4 == 0 {
            let slots = diff.storages.entry(address).or_default();
            for _ in 0..(3 + rng.gen_range(10)) {
                slots.insert(rng.b256(), U256::from(1 + rng.next_u64()));
            }
        }
    }
    diff
}

/// Generates a churn block over the existing state: balance changes, account deletions
/// (of storage-less accounts), fresh creations, storage writes and storage deletions.
fn gen_churn_diff(rng: &mut Rng, state: &PlainState) -> BlockDiff {
    let mut diff = BlockDiff::default();
    let addresses: Vec<_> = state.keys().copied().collect();

    for address in &addresses {
        let (account, storage) = &state[address];
        match rng.gen_range(8) {
            // Modify account info.
            0 => {
                diff.accounts.insert(*address, Some(rng.account()));
            }
            // Delete an EOA (no storage).
            1 if storage.is_empty() => {
                diff.accounts.insert(*address, None);
            }
            // Touch storage: overwrite an existing slot, delete another, add a new one.
            2 if !storage.is_empty() => {
                let slots: Vec<_> = storage.keys().copied().collect();
                let entry = diff.storages.entry(*address).or_default();
                entry.insert(slots[rng.gen_range(slots.len())], U256::from(1 + rng.next_u64()));
                entry.insert(slots[rng.gen_range(slots.len())], U256::ZERO);
                entry.insert(rng.b256(), U256::from(1 + rng.next_u64()));
                // Storage change implies the account trie leaf changes too (new storage
                // root), matching real execution where the account is in the bundle.
                diff.accounts.insert(*address, Some(*account));
            }
            // Selfdestruct a contract: account removed, every storage slot zeroed.
            3 if !storage.is_empty() => {
                diff.accounts.insert(*address, None);
                let entry = diff.storages.entry(*address).or_default();
                for slot in storage.keys() {
                    entry.insert(*slot, U256::ZERO);
                }
            }
            _ => {}
        }
    }

    // A few fresh creations, some with storage.
    for i in 0..4 {
        let address = rng.address();
        diff.accounts.insert(address, Some(rng.account()));
        if i % 2 == 0 {
            let slots = diff.storages.entry(address).or_default();
            for _ in 0..(1 + rng.gen_range(4)) {
                slots.insert(rng.b256(), U256::from(1 + rng.next_u64()));
            }
        }
    }

    diff
}

/// Writes one block to the database: changesets, hashed state, and incremental trie
/// updates. Returns the post-block state root.
fn write_block<P>(provider: &P, block: u64, state_before: &PlainState, diff: &BlockDiff) -> B256
where
    P: DBProvider<Tx: DbTxMut>
        + HashingWriter
        + TrieWriter
        + reth_storage_api::StorageSettingsCache,
{
    // Changesets record pre-block values.
    for address in diff.accounts.keys() {
        let pre = state_before.get(address).map(|(account, _)| *account);
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                block,
                AccountBeforeTx { address: *address, info: pre },
            )
            .unwrap();
    }
    for (address, slots) in &diff.storages {
        for slot in slots.keys() {
            let pre = state_before
                .get(address)
                .and_then(|(_, storage)| storage.get(slot).copied())
                .unwrap_or_default();
            provider
                .tx_ref()
                .put::<tables::StorageChangeSets>(
                    BlockNumberAddress((block, *address)),
                    StorageEntry { key: *slot, value: pre },
                )
                .unwrap();
        }
    }

    // Update hashed state to post-block values.
    provider.insert_account_for_hashing(diff.accounts.clone()).unwrap();
    provider
        .insert_storage_for_hashing(diff.storages.iter().map(|(address, slots)| {
            (*address, slots.iter().map(|(slot, value)| StorageEntry { key: *slot, value: *value }))
        }))
        .unwrap();

    // Incrementally update the trie, mirroring what the merkle stage / engine do.
    let mut account_prefix_set = PrefixSetMut::default();
    for address in diff.touched_addresses() {
        account_prefix_set.insert(Nibbles::unpack(keccak256(address)));
    }
    let mut storage_prefix_sets = B256Map::default();
    for (address, slots) in &diff.storages {
        let mut prefix_set = PrefixSetMut::default();
        for slot in slots.keys() {
            prefix_set.insert(Nibbles::unpack(keccak256(slot)));
        }
        storage_prefix_sets.insert(keccak256(address), prefix_set.freeze());
    }
    // Selfdestructed contracts (account deleted while owning storage) wipe their whole
    // storage trie, mirroring how the merkle stage handles destroyed accounts.
    let destroyed_accounts = diff
        .accounts
        .iter()
        .filter(|(address, account)| {
            account.is_none() &&
                state_before.get(*address).is_some_and(|(_, storage)| !storage.is_empty())
        })
        .map(|(address, _)| keccak256(address))
        .collect();

    let prefix_sets = TriePrefixSets {
        account_prefix_set: account_prefix_set.freeze(),
        storage_prefix_sets,
        destroyed_accounts,
    };

    let (root, updates) = reth_trie_db::with_adapter!(provider, |A| {
        DbStateRoot::<_, A>::from_tx(provider.tx_ref())
            .with_prefix_sets(prefix_sets)
            .root_with_updates()
            .unwrap()
    });
    provider.write_trie_updates(updates).unwrap();
    root
}

/// Builds the `HashedPostState` target for the witness request of `block`, mirroring
/// `ExecutionWitnessRecord::record_executed_state`: post-block values for every touched
/// account and slot (zero values included), plus a few read-only accounts.
fn witness_target(
    rng: &mut Rng,
    diff: &BlockDiff,
    state_after: &PlainState,
    state_before: &PlainState,
) -> HashedPostState {
    let mut target = HashedPostState::default();

    for address in diff.touched_addresses() {
        let post = state_after.get(address).map(|(account, _)| *account);
        target.accounts.insert(keccak256(address), post);
    }
    for (address, slots) in &diff.storages {
        // Selfdestructed accounts are recorded with a wiped storage, matching
        // `HashedStorage::new(account.status.was_destroyed())` in the witness record.
        let wiped = diff.accounts.get(address) == Some(&None) &&
            state_before.get(address).is_some_and(|(_, storage)| !storage.is_empty());
        let storage =
            target.storages.entry(keccak256(address)).or_insert_with(|| HashedStorage::new(wiped));
        for (slot, value) in slots {
            storage.storage.insert(keccak256(slot), *value);
        }
    }

    // Read-only accounts also end up in the record (they are in the state cache).
    let untouched: Vec<_> = state_before
        .iter()
        .filter(|(address, _)| !target.accounts.contains_key(&keccak256(address)))
        .collect();
    if !untouched.is_empty() {
        for _ in 0..3.min(untouched.len()) {
            let (address, (account, _)) = untouched[rng.gen_range(untouched.len())];
            target.accounts.insert(keccak256(address), Some(*account));
        }
    }

    target
}

/// Resolves an [`RlpNode`] child reference against the witness node map. Hash references
/// must be present in the witness; inline (< 32 byte) nodes are returned directly.
fn resolve_child(nodes: &B256Map<Bytes>, child: &RlpNode, context: &str) -> Result<Bytes, String> {
    if let Some(hash) = child.as_hash() {
        nodes.get(&hash).cloned().ok_or_else(|| format!("missing node {hash} ({context})"))
    } else {
        Ok(Bytes::copy_from_slice(child.as_ref()))
    }
}

/// Index of the child at `nibble` within a branch node's child stack.
const fn branch_child_index(branch: &BranchNode, nibble: u8) -> usize {
    (branch.state_mask.get() & ((1u16 << nibble) - 1)).count_ones() as usize
}

/// Walks the trie rooted at `root` towards `key` using only nodes from the witness.
/// Returns the leaf value if the key exists, `None` if the witness proves exclusion,
/// or an error naming the first missing node.
fn walk(
    nodes: &B256Map<Bytes>,
    root: B256,
    key: Nibbles,
    context: &str,
) -> Result<Option<Vec<u8>>, String> {
    if root == EMPTY_ROOT_HASH {
        return Ok(None);
    }
    let mut node_bytes =
        nodes.get(&root).cloned().ok_or_else(|| format!("missing root node {root} ({context})"))?;
    let mut pos = 0usize;
    loop {
        let node = TrieNode::decode(&mut &node_bytes[..])
            .map_err(|err| format!("undecodable node at depth {pos} ({context}): {err}"))?;
        match node {
            TrieNode::EmptyRoot => return Ok(None),
            TrieNode::Branch(branch) => {
                if pos == key.len() {
                    return Ok(None);
                }
                let nibble = key.get_unchecked(pos);
                if !branch.state_mask.is_bit_set(nibble) {
                    return Ok(None);
                }
                let child = &branch.stack[branch_child_index(&branch, nibble)];
                pos += 1;
                node_bytes = resolve_child(
                    nodes,
                    child,
                    &format!("{context}, branch child at depth {pos}"),
                )?;
            }
            TrieNode::Extension(extension) => {
                if key.slice(pos..).starts_with(&extension.key) {
                    pos += extension.key.len();
                    node_bytes = resolve_child(
                        nodes,
                        &extension.child,
                        &format!("{context}, extension child at depth {pos}"),
                    )?;
                } else {
                    return Ok(None);
                }
            }
            TrieNode::Leaf(leaf) => {
                return if key.slice(pos..) == leaf.key { Ok(Some(leaf.value)) } else { Ok(None) };
            }
        }
    }
}

/// Verifies that `witness` is complete for `target` against `parent_root`.
/// Returns a list of human-readable failures (empty when the witness is complete).
fn verify_witness_completeness(
    witness: &[Bytes],
    parent_root: B256,
    target: &HashedPostState,
) -> Vec<String> {
    let mut failures = Vec::new();
    let nodes: B256Map<Bytes> =
        witness.iter().map(|node| (keccak256(node), node.clone())).collect();

    if !nodes.contains_key(&parent_root) {
        failures.push(format!("witness does not contain parent state root node {parent_root}"));
        // Without the anchor node nothing below can be resolved either.
        return failures;
    }

    for hashed_address in target.accounts.keys() {
        let context = format!("account {hashed_address}");
        let account_leaf =
            match walk(&nodes, parent_root, Nibbles::unpack(hashed_address), &context) {
                Ok(leaf) => leaf,
                Err(err) => {
                    failures.push(err);
                    continue;
                }
            };

        // If the account had storage touched, its pre-block storage trie must be
        // resolvable down to every touched slot.
        let Some(storage) = target.storages.get(hashed_address) else { continue };
        let storage_root = match &account_leaf {
            Some(encoded) => match TrieAccount::decode(&mut encoded.as_slice()) {
                Ok(account) => account.storage_root,
                Err(err) => {
                    failures.push(format!("undecodable account leaf ({context}): {err}"));
                    continue;
                }
            },
            // Account did not exist in the parent state (created in this block); the
            // exclusion proof above is all that is required.
            None => continue,
        };
        for hashed_slot in storage.storage.keys() {
            let slot_context = format!("storage slot {hashed_slot} of account {hashed_address}");
            if let Err(err) =
                walk(&nodes, storage_root, Nibbles::unpack(hashed_slot), &slot_context)
            {
                failures.push(err);
            }
        }
    }

    failures
}

/// Full scenario: build a chain of `num_blocks` with per-block changesets and incremental
/// trie updates, then generate and verify a witness for every historical block.
fn run_historical_witness_scenario(seed: u64, num_blocks: u64, mode: ExecutionWitnessMode) {
    let mut rng = Rng(seed);
    let factory = create_test_provider_factory();
    let provider_rw = factory.provider_rw().unwrap();

    let mut state = PlainState::default();
    // roots[n] is the state root after block n (index 0 = genesis, empty state).
    let mut roots = vec![EMPTY_ROOT_HASH];
    // Per-block data needed to build witness targets later.
    let mut diffs = vec![BlockDiff::default()];
    let mut states_before = vec![PlainState::default()];

    for block in 1..=num_blocks {
        let diff = if block == 1 {
            gen_initial_diff(&mut rng, 500)
        } else {
            gen_churn_diff(&mut rng, &state)
        };
        states_before.push(state.clone());
        let root = write_block(&*provider_rw, block, &state, &diff);
        apply_diff(&mut state, &diff);

        // Cross-check the incrementally computed root against a reference implementation
        // so the test setup itself is known-good.
        let expected = state_root_prehashed(state.iter().map(|(address, (account, storage))| {
            (
                keccak256(address),
                (*account, storage.iter().map(|(slot, value)| (keccak256(slot), *value))),
            )
        }));
        assert_eq!(
            root, expected,
            "test setup produced wrong root for block {block} (seed {seed})"
        );

        roots.push(root);
        diffs.push(diff);
    }

    // HeaderNumbers mappings and the Finish checkpoint, so the historical provider can
    // resolve anchor hashes and the db tip. Headers themselves go to static files below,
    // after the database transaction commits.
    let headers: Vec<_> = (0..=num_blocks)
        .map(|block| Header {
            number: block,
            state_root: roots[block as usize],
            ..Default::default()
        })
        .collect();
    for header in &headers {
        provider_rw
            .tx_ref()
            .put::<tables::HeaderNumbers>(header.hash_slow(), header.number)
            .unwrap();
    }
    provider_rw.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(num_blocks)).unwrap();
    provider_rw.commit().unwrap();

    // Canonical headers in static files (kept separate from the database transaction:
    // committing a static file writer while a read-write provider is open deadlocks).
    let static_file_provider = factory.static_file_provider();
    let mut header_writer = static_file_provider.latest_writer(StaticFileSegment::Headers).unwrap();
    for header in &headers {
        header_writer.append_header(header, &header.hash_slow()).unwrap();
    }
    header_writer.commit().unwrap();
    drop(header_writer);

    // Request and verify a witness for every block below the tip. Block 1 is skipped:
    // its parent state is the empty genesis trie, for which canonical mode witnesses
    // legitimately contain no nodes.
    let provider = factory.provider().unwrap();
    let mut all_failures = Vec::new();
    for block in 2..num_blocks {
        let historical = HistoricalStateProviderRef::new(&provider, block, ChangesetCache::new());
        let mut state_after = states_before[block as usize].clone();
        apply_diff(&mut state_after, &diffs[block as usize]);
        let target = witness_target(
            &mut rng,
            &diffs[block as usize],
            &state_after,
            &states_before[block as usize],
        );

        let witness =
            historical.witness(TrieInput::default(), target.clone(), mode).unwrap_or_else(|err| {
                panic!("witness generation failed for block {block} (seed {seed}): {err}")
            });

        let parent_root = roots[block as usize - 1];
        for failure in verify_witness_completeness(&witness, parent_root, &target) {
            all_failures.push(format!("block {block} (seed {seed}, mode {mode:?}): {failure}"));
        }
    }

    assert!(
        all_failures.is_empty(),
        "incomplete historical witnesses:\n{}",
        all_failures.join("\n")
    );
}

#[test]
fn historical_witness_is_complete_legacy() {
    for seed in [2, 3, 5, 8, 13] {
        run_historical_witness_scenario(seed, 8, ExecutionWitnessMode::Legacy);
    }
}

#[test]
fn historical_witness_is_complete_canonical() {
    for seed in [2, 3, 5, 8, 13] {
        run_historical_witness_scenario(seed, 8, ExecutionWitnessMode::Canonical);
    }
}
