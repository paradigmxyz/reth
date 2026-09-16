//! Partitioned state root computation for full trie rebuilds.
//!
//! Rebuilds the state trie, including all storage tries, from the hashed state, building one
//! subtrie per first nibble in parallel. Computes the state root and emits every branch node that
//! belongs in the trie tables.

use alloy_primitives::{Bytes, B256, U256};
use alloy_rlp::Encodable;
use rayon::prelude::*;
use reth_primitives_traits::{Account, FastInstant as Instant};
use reth_provider::{DBProvider, DatabaseProviderROFactory, DbTxProvider, ProviderResult};
use reth_storage_errors::db::DatabaseError;
use reth_tasks::Runtime;
use reth_trie::hashed_cursor::{HashedCursor, HashedCursorFactory, HashedStorageCursor};
use reth_trie_common::{
    proof::ProofRetainer, BranchNodeCompact, HashBuilder, Nibbles, TrieNode, EMPTY_ROOT_HASH,
};
use reth_trie_db::DatabaseHashedCursorFactory;
use tracing::{debug, trace};

/// Default estimated number of slots above which a storage trie is built in parallel partitions.
pub const DEFAULT_STORAGE_PARTITION_THRESHOLD: u64 = 1 << 14;

/// The number of walked entries between two progress reports to the sink.
const PROGRESS_BATCH: u64 = 1 << 16;

/// `PartitionedStateRoot` computes the state root from the hashed state alone, in parallel
/// partitions.
///
/// The root and the emitted branch nodes are the same as those of a full rebuild with
/// [`StateRoot`](reth_trie::StateRoot).
///
/// The hashed state is read from a [`DatabaseProviderROFactory`] whose providers implement
/// [`HashedCursorFactory`], such as a database provider factory wrapped in [`HashedStateFactory`].
/// Each partition opens a provider of its own. Every branch node that belongs in the trie tables is
/// passed to a [`TrieSink`], along with the progress of the walk.
///
/// The account trie is split into sixteen partitions, one per first nibble, each built by its own
/// [`HashBuilder`] in parallel on the [`Runtime`]'s CPU pool. Large storage tries are partitioned
/// the same way.
#[derive(Debug)]
pub struct PartitionedStateRoot<S> {
    /// The source of the hashed state.
    source: S,
    /// The runtime whose CPU pool builds the partitions.
    runtime: Runtime,
    /// The estimated number of slots above which a storage trie is built in partitions.
    storage_partition_threshold: u64,
}

impl<S> PartitionedStateRoot<S> {
    /// Creates a new [`PartitionedStateRoot`] over `source`, building partitions on the CPU pool of
    /// `runtime`. The storage partition threshold is [`DEFAULT_STORAGE_PARTITION_THRESHOLD`].
    pub fn new(source: S, runtime: &Runtime) -> Self {
        Self {
            source,
            runtime: runtime.clone(),
            storage_partition_threshold: DEFAULT_STORAGE_PARTITION_THRESHOLD,
        }
    }

    /// Set the estimated number of slots above which a storage trie is built in partitions.
    ///
    /// The number of slots is estimated from the smallest hashed slot and is at least one, so a
    /// threshold of zero partitions every storage trie. Setting it to `u64::MAX` disables storage
    /// trie partitioning.
    pub const fn with_storage_partition_threshold(mut self, threshold: u64) -> Self {
        self.storage_partition_threshold = threshold;
        self
    }
}

impl<S> PartitionedStateRoot<S>
where
    S: DatabaseProviderROFactory<Provider: HashedCursorFactory> + Sync,
{
    /// Calculates the state root.
    pub fn root(self) -> ProviderResult<B256> {
        self.root_with_sink(&|_, _, _| {})
    }

    /// Calculates the state root and passes every branch node that belongs in the trie tables to
    /// `sink`.
    ///
    /// Nodes are passed once they are produced, concurrently from the partition workers and in no
    /// particular order. Nodes are not de-duplicated, so identical storage tries of different
    /// accounts are passed once per account.
    pub fn root_with_sink(self, sink: &impl TrieSink) -> ProviderResult<B256> {
        debug!(target: "trie::state_root", "Calculating state root");
        let started_at = Instant::now();

        let (root, leaves) = partitioned_root(self.runtime.cpu_pool(), |prefix| {
            self.build_account_subtrie(prefix, sink)
        })?;

        debug!(
            target: "trie::state_root",
            %root,
            duration = ?started_at.elapsed(),
            leaves,
            "Calculated state root"
        );
        Ok(root)
    }

    /// Builds the subtrie of all accounts under `prefix`, using a new provider.
    ///
    /// Passes the stored branch nodes of the subtrie and of its accounts' storage tries to `sink`.
    /// Returns `None` if no account starts with `prefix`.
    fn build_account_subtrie(
        &self,
        prefix: Nibbles,
        sink: &impl TrieSink,
    ) -> ProviderResult<Option<Subtrie>> {
        let started_at = Instant::now();
        let provider = self.source.database_provider_ro()?;
        let mut accounts = provider.hashed_account_cursor()?;
        let Some(first) = seek_prefix(&mut accounts, prefix)? else { return Ok(None) };
        // A single storage cursor per partition, repositioned for each account.
        let mut storage = provider.hashed_storage_cursor(first.0)?;
        // Each account leaf needs its storage root, so its storage trie is built before it is
        // encoded.
        let encoder = |hashed_address: B256, account: Account, rlp: &mut Vec<u8>| {
            let storage_root = self.storage_root(&mut storage, hashed_address, sink)?;
            account.into_trie_account(storage_root).encode(rlp);
            Ok(())
        };
        let subtrie = build_subtrie(&mut accounts, first, prefix, None, sink, encoder)?;

        trace!(
            target: "trie::state_root",
            ?prefix,
            duration = ?started_at.elapsed(),
            leaves = subtrie.leaves,
            "built account partition"
        );
        Ok(Some(subtrie))
    }

    /// Calculates the storage root of `hashed_address`, passing its branch nodes to `sink`.
    ///
    /// `cursor` is repositioned to the account. Small storage tries are built on `cursor`, large
    /// ones are built in partitions using new providers.
    fn storage_root(
        &self,
        cursor: &mut impl HashedStorageCursor<Value = U256>,
        hashed_address: B256,
        sink: &impl TrieSink,
    ) -> ProviderResult<B256> {
        cursor.set_hashed_address(hashed_address);
        let Some(first) = cursor.seek(B256::ZERO)? else { return Ok(EMPTY_ROOT_HASH) };
        let started_at = Instant::now();

        // Data in leaves is just the RLP-encoded value.
        let encoder = |_: B256, value: U256, rlp: &mut Vec<u8>| {
            value.encode(rlp);
            Ok(())
        };
        let partitioned = estimated_slots(first.0) > self.storage_partition_threshold;
        let (root, leaves) = if partitioned {
            partitioned_root(self.runtime.cpu_pool(), |prefix| {
                // Stacks a second read transaction on the account partition's thread, which
                // reth's MDBX environment allows (`MDBX_NOSTICKYTHREADS`).
                let provider = self.source.database_provider_ro()?;
                let mut cursor = provider.hashed_storage_cursor(hashed_address)?;
                let Some(first) = seek_prefix(&mut cursor, prefix)? else { return Ok(None) };
                build_subtrie(&mut cursor, first, prefix, Some(hashed_address), sink, encoder)
                    .map(Some)
            })?
        } else {
            // Build the whole trie on this cursor, starting from the slot already fetched.
            let subtrie =
                build_subtrie(cursor, first, Nibbles::new(), Some(hashed_address), sink, encoder)?;
            (subtrie.hash, subtrie.leaves)
        };

        trace!(
            target: "trie::storage_root",
            %root,
            %hashed_address,
            partitioned,
            duration = ?started_at.elapsed(),
            leaves,
            "calculated storage root"
        );
        Ok(root)
    }
}

/// A [`DatabaseProviderROFactory`] whose providers implement [`HashedCursorFactory`] over the
/// hashed state tables of a database provider factory.
///
/// The providers have long-lived read transaction safety, such as the transaction timeout,
/// disabled, since [`PartitionedStateRoot`] keeps each read transaction open for the whole build.
#[derive(Debug, Clone)]
pub struct HashedStateFactory<F>(F);

impl<F> HashedStateFactory<F> {
    /// Creates a new [`HashedStateFactory`] over `factory`.
    pub const fn new(factory: F) -> Self {
        Self(factory)
    }
}

impl<F> DatabaseProviderROFactory for HashedStateFactory<F>
where
    F: DatabaseProviderROFactory<Provider: DBProvider>,
{
    type Provider = HashedStateProvider<F::Provider>;

    fn database_provider_ro(&self) -> ProviderResult<Self::Provider> {
        Ok(HashedStateProvider(
            self.0.database_provider_ro()?.disable_long_read_transaction_safety(),
        ))
    }
}

/// A database provider implementing [`HashedCursorFactory`] over its transaction.
#[derive(Debug)]
pub struct HashedStateProvider<P>(P);

impl<P: DbTxProvider> HashedCursorFactory for HashedStateProvider<P> {
    type AccountCursor<'a>
        = <DatabaseHashedCursorFactory<&'a P::Tx> as HashedCursorFactory>::AccountCursor<'a>
    where
        Self: 'a;
    type StorageCursor<'a>
        = <DatabaseHashedCursorFactory<&'a P::Tx> as HashedCursorFactory>::StorageCursor<'a>
    where
        Self: 'a;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        DatabaseHashedCursorFactory::new(self.0.tx()).hashed_account_cursor()
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        DatabaseHashedCursorFactory::new(self.0.tx()).hashed_storage_cursor(hashed_address)
    }
}

/// A sink for the branch nodes that belong in the trie tables, and for the progress of the build.
pub trait TrieSink: Sync {
    /// Invoked with each branch node that belongs in the trie tables.
    ///
    /// `hashed_address` is `None` for the account trie and identifies the account of a storage
    /// trie. `path` is the full path of the node within that trie.
    fn on_branch_node(&self, hashed_address: Option<B256>, path: Nibbles, node: BranchNodeCompact);

    /// Invoked periodically with the number of hashed entries walked, for progress reporting.
    ///
    /// As with `hashed_entries_walked` in [`StateRootProgress`](reth_trie::StateRootProgress),
    /// entries are hashed accounts and storage slots, and each call reports only the entries walked
    /// since the previous report. The progress is the sum over all calls, which may come
    /// concurrently from different partitions. The default does nothing.
    fn on_progress(&self, _entries: u64) {}
}

impl<F> TrieSink for F
where
    F: Fn(Option<B256>, Nibbles, BranchNodeCompact) + Sync,
{
    fn on_branch_node(&self, hashed_address: Option<B256>, path: Nibbles, node: BranchNodeCompact) {
        self(hashed_address, path, node)
    }
}

/// A built subtrie.
#[derive(Debug)]
struct Subtrie {
    /// Hash of the root node.
    hash: B256,
    /// RLP-encoded root node. For a non-empty prefix, this is a leaf or an extension node.
    root_rlp: Bytes,
    /// The number of leaves in the subtrie.
    leaves: u64,
}

/// Returns the root hash and the number of leaves of a trie whose first-nibble partitions are
/// built in parallel on `pool` by `build`.
fn partitioned_root(
    pool: &rayon::ThreadPool,
    build: impl Fn(Nibbles) -> ProviderResult<Option<Subtrie>> + Sync,
) -> ProviderResult<(B256, u64)> {
    // Collecting into a `Vec` preserves the iterator order, so the subtries are in key order.
    // Called from one of the pool's own workers, as for storage tries, `install` runs inline.
    let subtries: Vec<Subtrie> = pool.install(|| {
        (0u8..16)
            .into_par_iter()
            .filter_map(|nibble| build(Nibbles::from_nibbles([nibble])).transpose())
            .collect::<Result<_, _>>()
    })?;
    let leaves = subtries.iter().map(|subtrie| subtrie.leaves).sum();
    Ok((assemble_root(&subtries), leaves))
}

/// Estimates the number of slots in a storage trie from its smallest hashed slot.
///
/// With uniformly distributed hashes, the smallest of `n` slots is about `2^256 / n`. The estimate
/// is noisy, but sufficient: a trie with 1/16 of the threshold's slots is partitioned with a
/// probability of about 6%, costing 16 additional seeks, and a trie with 16 times as many slots is
/// built serially with a probability of `e^-16`.
fn estimated_slots(smallest_slot: B256) -> u64 {
    let leading_zeros = U256::from_be_bytes(smallest_slot.0).leading_zeros();
    1u64.checked_shl(leading_zeros as u32).unwrap_or(u64::MAX)
}

/// Returns the first entry on `cursor` whose key starts with `prefix`, if any.
fn seek_prefix<C: HashedCursor>(
    cursor: &mut C,
    prefix: Nibbles,
) -> Result<Option<(B256, C::Value)>, DatabaseError> {
    // The smallest key with the given prefix is the packed prefix padded with zeros.
    let entry = cursor.seek(B256::right_padding_from(&prefix.pack()))?;
    Ok(entry.filter(|(key, _)| Nibbles::unpack_array(&key.0).starts_with(&prefix)))
}

/// Builds the subtrie from `first` and all following entries on `cursor` that start with
/// `prefix`, passing every stored branch node of the trie `hashed_address` to `sink`, along with
/// the number of entries walked.
///
/// `encoder` RLP-encodes an entry's value into the given buffer.
fn build_subtrie<C: HashedCursor>(
    cursor: &mut C,
    first: (B256, C::Value),
    prefix: Nibbles,
    hashed_address: Option<B256>,
    sink: &impl TrieSink,
    mut encoder: impl FnMut(B256, C::Value, &mut Vec<u8>) -> ProviderResult<()>,
) -> ProviderResult<Subtrie> {
    let mut hb = HashBuilder::default()
        .with_updates(true)
        // Without targets, only the root node is retained. This is the only way to get its RLP.
        .with_proof_retainer(ProofRetainer::new(Vec::new()));
    let mut rlp = Vec::new();
    let mut leaves: u64 = 0;
    let mut entry = Some(first);
    while let Some((key, value)) = entry {
        let path = Nibbles::unpack_array(&key.0);
        if !path.starts_with(&prefix) {
            break;
        }
        rlp.clear();
        encoder(key, value, &mut rlp)?;
        hb.add_leaf(path, &rlp);
        leaves += 1;
        emit_branch_nodes(&mut hb, hashed_address, sink);
        if leaves.is_multiple_of(PROGRESS_BATCH) {
            sink.on_progress(PROGRESS_BATCH);
        }
        entry = cursor.next()?;
    }
    let hash = hb.root();
    emit_branch_nodes(&mut hb, hashed_address, sink);
    // The leaves since the last full batch.
    sink.on_progress(leaves % PROGRESS_BATCH);
    let root_rlp = hb
        .take_proof_nodes()
        .into_inner()
        .remove(&Nibbles::new())
        .expect("retainer keeps the root node");

    Ok(Subtrie { hash, root_rlp, leaves })
}

/// Passes the branch nodes completed so far to `sink` and removes them from the builder.
///
/// This bounds the memory of the builder by the depth of the trie rather than its size.
fn emit_branch_nodes(hb: &mut HashBuilder, hashed_address: Option<B256>, sink: &impl TrieSink) {
    let completed = hb.updated_branch_nodes.as_mut().expect("updates are enabled");
    if completed.is_empty() {
        return;
    }
    // Nodes in `updated_branch_nodes` are final. `drain` keeps the map's allocation.
    for (path, node) in completed.drain() {
        // The root node is never stored. `TrieUpdates::finalize` removes it as well.
        if !path.is_empty() {
            sink.on_branch_node(hashed_address, path, node);
        }
    }
}

/// Calculates the root hash from the first-nibble subtries, which must be in key order.
fn assemble_root(subtries: &[Subtrie]) -> B256 {
    // Updates are not enabled, since the only node formed here is the root, which is never stored.
    // With longer prefixes, this could also form branch nodes that belong in the trie tables.
    let mut hb = HashBuilder::default();
    for subtrie in subtries {
        match alloy_rlp::decode_exact(&subtrie.root_rlp).expect("builder emits valid node RLP") {
            TrieNode::Leaf(leaf) => hb.add_leaf(leaf.key, &leaf.value),
            TrieNode::Extension(ext) => match ext.child.as_hash() {
                // `stored_in_database` is irrelevant, since the parent of the branch node is the
                // root node, which is never stored.
                Some(hash) => hb.add_branch(ext.key, hash, false),
                // Highly unlikely, but the child may be inlined if its RLP is under 32 bytes.
                None => add_inline_leaves(&mut hb, ext.key, &ext.child),
            },
            _ => unreachable!("partition root is a leaf or an extension"),
        }
    }
    hb.root()
}

/// Decodes the RLP-encoded inline `node` at `path` and adds all its leaves to `hb`.
///
/// [`HashBuilder::add_branch`] only accepts a hash, so an inline node is added through its leaves
/// instead.
fn add_inline_leaves(hb: &mut HashBuilder, path: Nibbles, node: &[u8]) {
    match alloy_rlp::decode_exact(node).expect("inline node is valid RLP") {
        TrieNode::Leaf(leaf) => hb.add_leaf(path.join(&leaf.key), &leaf.value),
        TrieNode::Extension(ext) => add_inline_leaves(hb, path.join(&ext.key), &ext.child),
        TrieNode::Branch(branch) => {
            for (nibble, child) in branch.as_ref().children() {
                if let Some(child) = child {
                    let mut path = path;
                    path.push(nibble);
                    add_inline_leaves(hb, path, child);
                }
            }
        }
        TrieNode::EmptyRoot => unreachable!("inline child is not empty"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{
        collection::btree_map,
        prelude::{ProptestConfig, Strategy},
        proptest,
    };
    use proptest_arbitrary_interop::arb;
    use reth_provider::{test_utils::create_test_provider_factory, StateWriter};
    use reth_trie::{
        hashed_cursor::mock::MockHashedCursorFactory, trie_cursor::noop::NoopTrieCursorFactory,
        HashedPostState, HashedStorage, StateRoot,
    };
    use std::{collections::BTreeMap, sync::Mutex};

    type State = BTreeMap<B256, (Account, BTreeMap<B256, U256>)>;
    type Node = (Option<B256>, Nibbles, BranchNodeCompact);

    /// Serves every partition with a clone of the same in-memory cursor factory.
    #[derive(Clone)]
    struct SharedFactory(MockHashedCursorFactory);

    impl DatabaseProviderROFactory for SharedFactory {
        type Provider = MockHashedCursorFactory;

        fn database_provider_ro(&self) -> ProviderResult<Self::Provider> {
            Ok(self.0.clone())
        }
    }

    fn cursor_factory(state: &State) -> MockHashedCursorFactory {
        let accounts = state.iter().map(|(address, (account, _))| (*address, *account)).collect();
        let storages =
            state.iter().map(|(address, (_, storage))| (*address, storage.clone())).collect();
        MockHashedCursorFactory::new(accounts, storages)
    }

    /// Sorts nodes by trie and path.
    fn sorted(mut nodes: Vec<Node>) -> Vec<Node> {
        nodes.sort_by_key(|(hashed_address, path, _)| (*hashed_address, *path));
        nodes
    }

    /// Returns the root and the sorted branch nodes of reth's serial [`StateRoot`] over
    /// `factory`, with no trie tables to read from.
    fn reth_state_root(factory: impl HashedCursorFactory + Clone) -> (B256, Vec<Node>) {
        // Without trie tables to read from, all nodes are in the updates.
        let (root, updates) =
            StateRoot::new(NoopTrieCursorFactory::default(), factory).root_with_updates().unwrap();
        let account_nodes =
            updates.account_nodes.into_iter().map(|(path, node)| (None, path, node));
        let storage_nodes = updates.storage_tries.into_iter().flat_map(|(hashed_address, trie)| {
            trie.storage_nodes
                .into_iter()
                .map(move |(path, node)| (Some(hashed_address), path, node))
        });
        (root, sorted(account_nodes.chain(storage_nodes).collect()))
    }

    /// Runs `calculator` with a sink that collects all nodes, and returns the root and the
    /// sorted nodes.
    fn partitioned_state_root<S>(calculator: PartitionedStateRoot<S>) -> (B256, Vec<Node>)
    where
        S: DatabaseProviderROFactory<Provider: HashedCursorFactory> + Sync,
    {
        let nodes = Mutex::new(Vec::new());
        let sink =
            |hashed_address, path, node| nodes.lock().unwrap().push((hashed_address, path, node));
        let root = calculator.root_with_sink(&sink).unwrap();
        (root, sorted(nodes.into_inner().unwrap()))
    }

    /// Asserts that [`PartitionedStateRoot`] over `source` yields the `expected` root and branch
    /// nodes, with all storage tries built serially, then all in partitions.
    fn assert_matches<S>(runtime: &Runtime, source: S, expected: &(B256, Vec<Node>))
    where
        S: DatabaseProviderROFactory<Provider: HashedCursorFactory> + Clone + Sync,
    {
        for threshold in [u64::MAX, 0] {
            let calculator = PartitionedStateRoot::new(source.clone(), runtime)
                .with_storage_partition_threshold(threshold);
            assert_eq!(&partitioned_state_root(calculator), expected, "threshold {threshold}");
        }
    }

    /// Asserts that the root and the branch nodes of `state` match those of reth's serial
    /// [`StateRoot`] over the same in-memory cursors.
    fn assert_matches_reth(runtime: &Runtime, state: &State) {
        let factory = cursor_factory(state);
        let expected = reth_state_root(factory.clone());
        assert_matches(runtime, SharedFactory(factory), &expected);
    }

    fn arb_state() -> impl Strategy<Value = State> {
        // `arb::<State>()` alone mostly yields states with no more than a few entries.
        let storage = btree_map(arb::<B256>(), arb::<U256>(), 0..50);
        btree_map(arb::<B256>(), (arb::<Account>(), storage), 0..90)
    }

    #[test]
    fn arbitrary_state_root() {
        let runtime = Runtime::test();
        proptest!(ProptestConfig::with_cases(10), |(state in arb_state())| {
            assert_matches_reth(&runtime, &state);
        });
    }

    #[test]
    fn edge_case_storage_root() {
        let runtime = Runtime::test();
        let key = B256::right_padding_from;
        let cases: [Vec<B256>; 6] = [
            // No slots, so the root is the empty root.
            vec![],
            // A single slot, so the root is a leaf.
            vec![key(&[0x42])],
            // Two partitions with a leaf each, so the root is a branch with two leaf children.
            vec![key(&[0x10]), key(&[0xa0])],
            // A single partition, whose root is an extension.
            vec![key(&[0x71, 0x23]), key(&[0x71, 0x24]), key(&[0x71, 0x30])],
            // Partition 0 is an extension to an inline branch, next to another partition.
            vec![B256::with_last_byte(1), B256::with_last_byte(2), key(&[0x80])],
            // Every partition holds 16 slots, so their roots are extensions to branches.
            (0u8..=255).map(|byte| key(&[byte, byte.wrapping_mul(7)])).collect(),
        ];

        for keys in cases {
            // Small values keep the leaves short, so that nodes can be inlined.
            let storage =
                keys.into_iter().zip(1u64..).map(|(key, value)| (key, U256::from(value))).collect();
            // The cases target the assembly of the partitions.
            let state = State::from([(B256::ZERO, (Account::default(), storage))]);
            assert_matches_reth(&runtime, &state);
        }
    }

    #[test]
    fn database_state_root() {
        let runtime = Runtime::test();
        proptest!(ProptestConfig::with_cases(10), |(state in arb_state())| {
            let factory = create_test_provider_factory();
            let hashed_state: HashedPostState = state
                .iter()
                .map(|(address, (account, storage))| {
                    let storage = HashedStorage::from_iter(storage.clone());
                    (*address, Some(*account), (!storage.is_empty()).then_some(storage))
                })
                .collect();
            let provider = factory.provider_rw().unwrap();
            provider.write_hashed_state(&hashed_state.into_sorted()).unwrap();
            provider.commit().unwrap();

            // Both run over the MDBX tables, so the reference is taken from them as well.
            let expected = reth_state_root(DatabaseHashedCursorFactory::new(
                factory.provider().unwrap().tx_ref(),
            ));
            assert_matches(&runtime, HashedStateFactory::new(factory), &expected);
        });
    }
}
