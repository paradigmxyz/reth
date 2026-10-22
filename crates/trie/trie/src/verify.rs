use crate::{
    hashed_cursor::{HashedCursor, HashedCursorFactory, HashedStorageCursor},
    trie_cursor::{
        depth_first::{self, DepthFirstTrieIterator},
        noop::NoopTrieCursorFactory,
        TrieCursor, TrieCursorFactory, TrieStorageCursor,
    },
    Nibbles,
};
use alloy_primitives::{map::B256Map, B256};
use alloy_rlp::Encodable;
use alloy_trie::BranchNodeCompact;
use reth_execution_errors::StateRootError;
use reth_storage_errors::db::DatabaseError;
use std::{
    cmp::Ordering,
    collections::VecDeque,
    panic::{catch_unwind, AssertUnwindSafe},
    thread::{self, JoinHandle},
};
use tracing::trace;

/// Output describes an inconsistency found when comparing the hashed state tables
/// ([`HashedCursorFactory`]) with that of the trie tables ([`TrieCursorFactory`]). The hashed
/// tables are considered the source of truth; outputs are on the part of the trie tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// An extra account node was found.
    AccountExtra(Nibbles, BranchNodeCompact),
    /// An extra storage node was found.
    StorageExtra(B256, Nibbles, BranchNodeCompact),
    /// An account node had the wrong value.
    AccountWrong {
        /// Path of the node
        path: Nibbles,
        /// The node's expected value.
        expected: BranchNodeCompact,
        /// The node's found value.
        found: BranchNodeCompact,
    },
    /// A storage node had the wrong value.
    StorageWrong {
        /// The account the storage trie belongs to.
        account: B256,
        /// Path of the node
        path: Nibbles,
        /// The node's expected value.
        expected: BranchNodeCompact,
        /// The node's found value.
        found: BranchNodeCompact,
    },
    /// An account node was missing.
    AccountMissing(Nibbles, BranchNodeCompact),
    /// A storage node was missing.
    StorageMissing(B256, Nibbles, BranchNodeCompact),
    /// Progress indicator with the last seen account path.
    Progress(Nibbles),
}

/// Verifies the contents of a trie table against some other data source which is able to produce
/// stored trie nodes.
#[derive(Debug)]
struct SingleVerifier<I> {
    account: Option<B256>, // None for accounts trie
    trie_iter: I,
    curr: Option<(Nibbles, BranchNodeCompact)>,
}

impl<C: TrieCursor> SingleVerifier<DepthFirstTrieIterator<C>> {
    fn new(account: Option<B256>, trie_cursor: C) -> Result<Self, DatabaseError> {
        let mut trie_iter = DepthFirstTrieIterator::new(trie_cursor);
        let curr = trie_iter.next().transpose()?;
        Ok(Self { account, trie_iter, curr })
    }

    const fn output_extra(&self, path: Nibbles, node: BranchNodeCompact) -> Output {
        if let Some(account) = self.account {
            Output::StorageExtra(account, path, node)
        } else {
            Output::AccountExtra(path, node)
        }
    }

    const fn output_wrong(
        &self,
        path: Nibbles,
        expected: BranchNodeCompact,
        found: BranchNodeCompact,
    ) -> Output {
        if let Some(account) = self.account {
            Output::StorageWrong { account, path, expected, found }
        } else {
            Output::AccountWrong { path, expected, found }
        }
    }

    const fn output_missing(&self, path: Nibbles, node: BranchNodeCompact) -> Output {
        if let Some(account) = self.account {
            Output::StorageMissing(account, path, node)
        } else {
            Output::AccountMissing(path, node)
        }
    }

    /// Called with the next path and node in the canonical sequence of stored trie nodes. Will
    /// append to the given `outputs` Vec if walking the trie cursor produces data
    /// inconsistent with that given.
    ///
    /// `next` must be called with paths in depth-first order.
    fn next(
        &mut self,
        outputs: &mut Vec<Output>,
        path: Nibbles,
        node: BranchNodeCompact,
    ) -> Result<(), DatabaseError> {
        loop {
            // `curr` is None only if the end of the iterator has been reached. Any further nodes
            // found must be considered missing.
            if self.curr.is_none() {
                outputs.push(self.output_missing(path, node));
                return Ok(())
            }

            let (curr_path, curr_node) = self.curr.as_ref().expect("not None");
            trace!(target: "trie::verify", account=?self.account, ?curr_path, ?path, "Current cursor node");

            // Use depth-first ordering for comparison
            match depth_first::cmp(&path, curr_path) {
                Ordering::Less => {
                    // If the given path comes before the cursor's current path in depth-first
                    // order, then the given path was not produced by the cursor.
                    outputs.push(self.output_missing(path, node));
                    return Ok(())
                }
                Ordering::Equal => {
                    // If the current path matches the given one (happy path) but the nodes
                    // aren't equal then we produce a wrong node. Either way we want to move the
                    // iterator forward.
                    if *curr_node != node {
                        outputs.push(self.output_wrong(path, node, curr_node.clone()))
                    }
                    self.curr = self.trie_iter.next().transpose()?;
                    return Ok(())
                }
                Ordering::Greater => {
                    // If the given path comes after the current path in depth-first order,
                    // it means the cursor's path was not found by the caller (otherwise it would
                    // have hit the equal case) and so is extraneous.
                    outputs.push(self.output_extra(*curr_path, curr_node.clone()));
                    self.curr = self.trie_iter.next().transpose()?;
                    // back to the top of the loop to check the latest `self.curr` value against the
                    // given path/node.
                }
            }
        }
    }

    /// Must be called once there are no more calls to `next` to made. All further nodes produced
    /// by the iterator will be considered extraneous.
    fn finalize(&mut self, outputs: &mut Vec<Output>) -> Result<(), DatabaseError> {
        loop {
            if let Some((curr_path, curr_node)) = self.curr.take() {
                outputs.push(self.output_extra(curr_path, curr_node));
                self.curr = self.trie_iter.next().transpose()?;
            } else {
                return Ok(())
            }
        }
    }
}

/// Checks trie tables against hashed state, recomputing storage roots on a fixed sync pool.
///
/// Cursor factories must provide long-lived read snapshots of the same database version. The
/// lookahead thread and storage workers own clones of the factories and run until EOF or drop.
/// Completed roots accumulate without a limit while account leaves are consumed in order.
/// Inconsistencies from every thread are delivered through the same queue.
#[derive(Debug)]
pub struct Verifier<'a, T: TrieCursorFactory + 'a, H> {
    hashed_cursor_factory: H,
    account: SingleVerifier<DepthFirstTrieIterator<T::AccountTrieCursor<'a>>>,
    hash_builder: crate::HashBuilder,
    last_account: Option<B256>,
    roots_rx: crossbeam_channel::Receiver<(B256, B256)>,
    roots: B256Map<B256>,
    events_tx: crossbeam_channel::Sender<Result<Output, StateRootError>>,
    events_rx: crossbeam_channel::Receiver<Result<Output, StateRootError>>,
    outputs: VecDeque<Result<Output, StateRootError>>,
    cancel: Option<crossbeam_channel::Sender<()>>,
    background: Option<JoinHandle<()>>,
    complete: bool,
}

/// Defaults to the available CPU count, or one worker if it cannot be determined.
pub fn default_storage_workers() -> usize {
    thread::available_parallelism().map_or(1, usize::from)
}

const ACCOUNT_BATCH_SIZE: usize = 16_384;
const ACCOUNT_UPDATE_THRESHOLD: usize = 1024;

impl<'a, T, H> Verifier<'a, T, H>
where
    T: TrieCursorFactory + Clone + Send + Sync + 'static,
    H: HashedCursorFactory + Clone + Send + Sync + 'static,
{
    /// Starts continuous storage verification over a fixed database state. Factories must own
    /// their snapshots so background work can continue between iterator calls. Dropping the
    /// verifier cancels and joins all background work before returning.
    pub fn new(
        trie_cursor_factory: &'a T,
        hashed_cursor_factory: H,
    ) -> Result<Self, DatabaseError> {
        let account = SingleVerifier::new(None, trie_cursor_factory.account_trie_cursor()?)?;
        let trie = trie_cursor_factory.clone();
        let hashed = hashed_cursor_factory.clone();
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let (roots_tx, roots_rx) = crossbeam_channel::unbounded();
        let (cancel_tx, cancel_rx) = crossbeam_channel::bounded::<()>(0);
        let events = events_tx.clone();
        let span = tracing::Span::current();
        let background = thread::Builder::new()
            .name("trie-lookahead".into())
            .spawn(move || {
                let _entered = span.enter();
                run_verification(&events, &cancel_rx, || {
                    let workers = default_storage_workers();
                    let pool = reth_tasks::WorkerPool::new(workers, "trie-verify");
                    // Only queued work is bounded; completed roots never wait for the account
                    // thread.
                    let (jobs_tx, jobs_rx) = crossbeam_channel::bounded(workers * 2);
                    pool.in_place_scope(|scope| {
                        for _ in 0..workers {
                            let jobs = jobs_rx.clone();
                            let span = tracing::Span::current();
                            let (trie, hashed, events, roots, cancel) =
                                (&trie, &hashed, &events, &roots_tx, &cancel_rx);
                            scope.spawn(move |_| {
                                let _entered = span.enter();
                                run_verification(events, cancel, || {
                                    storage_worker(trie, hashed, jobs, roots, events, cancel)
                                });
                            });
                        }
                        run_verification(&events, &cancel_rx, || {
                            produce_accounts(&hashed, jobs_tx, &cancel_rx)
                        });
                    });
                    Ok(())
                });
            })
            .map_err(|error| DatabaseError::Other(error.to_string()))?;
        Ok(Self {
            hashed_cursor_factory,
            account,
            hash_builder: crate::HashBuilder::default().with_updates(true),
            last_account: None,
            roots_rx,
            roots: Default::default(),
            events_tx,
            events_rx,
            outputs: Default::default(),
            cancel: Some(cancel_tx),
            background: Some(background),
            complete: false,
        })
    }

    fn storage_root(&mut self, address: B256) -> Result<B256, StateRootError> {
        if let Some(root) = self.roots.remove(&address) {
            return Ok(root)
        }
        loop {
            crossbeam_channel::select! {
                recv(self.events_rx) -> event => {
                    if let Ok(event) = event { self.outputs.push_back(Ok(event?)); }
                }
                recv(self.roots_rx) -> root => {
                    let Ok((account, root)) = root else {
                        for event in self.events_rx.try_iter() {
                            self.outputs.push_back(Ok(event?));
                        }
                        return Err(DatabaseError::Other("Storage root workers disconnected".into()).into())
                    };
                    if account == address { return Ok(root) }
                    self.roots.insert(account, root);
                }
            }
        }
    }

    fn fill_batch(&mut self) -> Result<(), StateRootError> {
        let hashed = self.hashed_cursor_factory.clone();
        let mut cursor = hashed.hashed_account_cursor()?;
        let mut next = if let Some(last) = self.last_account {
            cursor.seek(last)?;
            cursor.next()?
        } else {
            cursor.seek(B256::ZERO)?
        };
        let Some((first, _)) = next else { return self.finish() };
        let mut storage = hashed.hashed_storage_cursor(first)?;
        let mut account_rlp = Vec::with_capacity(crate::TRIE_ACCOUNT_RLP_MAX_SIZE);
        for _ in 0..ACCOUNT_BATCH_SIZE {
            let Some((address, account)) = next else { break };
            for event in self.events_rx.try_iter() {
                self.outputs.push_back(Ok(event?));
            }
            storage.set_hashed_address(address);
            let root = if storage.is_storage_empty()? {
                alloy_consensus::EMPTY_ROOT_HASH
            } else {
                self.storage_root(address)?
            };
            account_rlp.clear();
            account.into_trie_account(root).encode(&mut account_rlp);
            self.hash_builder.add_leaf(Nibbles::unpack(address), &account_rlp);
            self.last_account = Some(address);
            if self.hash_builder.updates_len() >= ACCOUNT_UPDATE_THRESHOLD {
                self.verify_account_updates()?;
            }
            next = cursor.next()?;
        }
        if next.is_none() {
            self.finish()?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), StateRootError> {
        // Joining before cancellation preserves errors from the producer's final cursor read.
        self.join_background();
        self.cancel.take();
        self.complete = true;
        self.hash_builder.root();
        self.verify_account_updates()?;
        let mut found = Vec::new();
        self.account.finalize(&mut found)?;
        report_outputs(found, &self.events_tx, &self.events_rx, &mut self.outputs)
    }

    fn verify_account_updates(&mut self) -> Result<(), StateRootError> {
        let (builder, updates) = std::mem::take(&mut self.hash_builder).split();
        self.hash_builder = builder.with_updates(true);
        // The account trie root is calculated but never persisted.
        let mut nodes =
            updates.into_iter().filter(|(path, _)| !path.is_empty()).collect::<Vec<_>>();
        nodes.sort_unstable_by(|a, b| depth_first::cmp(&a.0, &b.0));
        let mut found = Vec::new();
        for (path, node) in nodes {
            self.account.next(&mut found, path, node)?;
            found.push(Output::Progress(path));
        }
        report_outputs(found, &self.events_tx, &self.events_rx, &mut self.outputs)
    }
}

impl<T: TrieCursorFactory, H> Verifier<'_, T, H> {
    fn join_background(&mut self) {
        if let Some(background) = self.background.take() &&
            let Err(panic) = background.join() &&
            !thread::panicking()
        {
            std::panic::resume_unwind(panic);
        }
    }
}

impl<T: TrieCursorFactory, H> Drop for Verifier<'_, T, H> {
    fn drop(&mut self) {
        self.cancel.take();
        self.join_background();
    }
}

impl<T, H> Iterator for Verifier<'_, T, H>
where
    T: TrieCursorFactory + Clone + Send + Sync + 'static,
    H: HashedCursorFactory + Clone + Send + Sync + 'static,
{
    type Item = Result<Output, StateRootError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(output) = self.outputs.pop_front() {
                return Some(output)
            }
            if self.complete {
                return None
            }
            if let Err(error) = self.fill_batch() {
                self.complete = true;
                self.cancel.take();
                self.join_background();
                self.outputs.push_back(Err(error));
            }
        }
    }
}

fn report_outputs(
    found: Vec<Output>,
    sender: &crossbeam_channel::Sender<Result<Output, StateRootError>>,
    receiver: &crossbeam_channel::Receiver<Result<Output, StateRootError>>,
    outputs: &mut VecDeque<Result<Output, StateRootError>>,
) -> Result<(), StateRootError> {
    for output in found {
        assert!(sender.send(Ok(output)).is_ok(), "verifier owns the event receiver");
    }
    for queued in receiver.try_iter() {
        outputs.push_back(Ok(queued?));
    }
    Ok(())
}

fn send_verification<T>(
    sender: &crossbeam_channel::Sender<T>,
    value: T,
    cancel: &crossbeam_channel::Receiver<()>,
) -> bool {
    crossbeam_channel::select! {
        send(sender, value) -> result => result.is_ok(),
        recv(cancel) -> _ => false,
    }
}

fn run_verification(
    events: &crossbeam_channel::Sender<Result<Output, StateRootError>>,
    cancel: &crossbeam_channel::Receiver<()>,
    f: impl FnOnce() -> Result<(), StateRootError>,
) {
    let result = catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        Err(DatabaseError::Other("Trie verification thread panicked".into()).into())
    });
    if let Err(error) = result {
        send_verification(events, Err(error), cancel);
    }
}

fn produce_accounts<H: HashedCursorFactory>(
    hashed: &H,
    jobs: crossbeam_channel::Sender<B256>,
    cancel: &crossbeam_channel::Receiver<()>,
) -> Result<(), StateRootError> {
    let mut cursor = hashed.hashed_account_cursor()?;
    let mut next = cursor.seek(B256::ZERO)?;
    while let Some((address, _)) = next {
        if !send_verification(&jobs, address, cancel) {
            return Ok(())
        }
        next = cursor.next()?;
    }
    Ok(())
}

fn storage_worker<T, H>(
    trie: &T,
    hashed: &H,
    jobs: crossbeam_channel::Receiver<B256>,
    roots: &crossbeam_channel::Sender<(B256, B256)>,
    events: &crossbeam_channel::Sender<Result<Output, StateRootError>>,
    cancel: &crossbeam_channel::Receiver<()>,
) -> Result<(), StateRootError>
where
    T: TrieCursorFactory,
    H: HashedCursorFactory + Clone,
{
    let next_address = || {
        crossbeam_channel::select! {
            recv(jobs) -> job => job,
            recv(cancel) -> _ => Err(crossbeam_channel::RecvError),
        }
    };
    let Ok(mut address) = next_address() else { return Ok(()) };
    let mut storage = hashed.hashed_storage_cursor(address)?;
    let mut stored = trie.storage_trie_cursor(address)?;
    loop {
        storage.set_hashed_address(address);
        if storage.is_storage_empty()? {
            stored.set_hashed_address(address);
            let mut verifier = SingleVerifier::new(Some(address), &mut stored)?;
            let mut found = Vec::new();
            verifier.finalize(&mut found)?;
            for output in found {
                if !send_verification(events, Ok(output), cancel) {
                    return Ok(())
                }
            }
        } else {
            let Some(root) = verify_storage(trie, hashed, address, events, cancel)? else {
                return Ok(())
            };
            if !send_verification(roots, (address, root), cancel) {
                return Ok(())
            }
        }
        let Ok(next) = next_address() else { return Ok(()) };
        address = next;
    }
}

fn verify_storage<T, H>(
    trie: &T,
    hashed: &H,
    address: B256,
    events: &crossbeam_channel::Sender<Result<Output, StateRootError>>,
    cancel: &crossbeam_channel::Receiver<()>,
) -> Result<Option<B256>, StateRootError>
where
    T: TrieCursorFactory,
    H: HashedCursorFactory + Clone,
{
    let mut verifier = SingleVerifier::new(Some(address), trie.storage_trie_cursor(address)?)?;
    let mut state = None;
    loop {
        if cancel.try_recv() == Err(crossbeam_channel::TryRecvError::Disconnected) {
            return Ok(None)
        }
        let result = crate::StorageRoot::new_hashed(
            NoopTrieCursorFactory,
            hashed.clone(),
            address,
            Default::default(),
            #[cfg(feature = "metrics")]
            Default::default(),
        )
        .with_intermediate_state(state.take())
        .root_with_progress()?;
        let (root, updates) = match result {
            crate::StorageRootProgress::Complete(root, _, updates) => (Some(root), updates),
            crate::StorageRootProgress::Progress(next, _, updates) => {
                state = Some(*next);
                (None, updates)
            }
        };
        let mut nodes = updates.storage_nodes.into_iter().collect::<Vec<_>>();
        nodes.sort_unstable_by(|a, b| depth_first::cmp(&a.0, &b.0));
        let mut found = Vec::new();
        for (path, node) in nodes {
            verifier.next(&mut found, path, node)?;
        }
        if root.is_some() {
            verifier.finalize(&mut found)?;
        }
        for output in found {
            if !send_verification(events, Ok(output), cancel) {
                return Ok(None)
            }
        }
        if root.is_some() {
            return Ok(root)
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::{
        hashed_cursor::mock::MockHashedCursorFactory,
        trie_cursor::mock::{MockTrieCursor, MockTrieCursorFactory},
    };
    use alloy_primitives::{keccak256, map::B256Map, Address, U256};
    use alloy_trie::TrieMask;
    use assert_matches::assert_matches;
    use reth_primitives_traits::Account;
    use std::collections::BTreeMap;

    /// Helper function to create a simple test `BranchNodeCompact`
    fn test_branch_node(
        state_mask: u16,
        tree_mask: u16,
        hash_mask: u16,
        hashes: Vec<B256>,
    ) -> BranchNodeCompact {
        // Ensure the number of hashes matches the number of bits set in hash_mask
        let expected_hashes = hash_mask.count_ones() as usize;
        let mut final_hashes = hashes;
        let mut counter = 100u8;
        while final_hashes.len() < expected_hashes {
            final_hashes.push(B256::repeat_byte(counter));
            counter += 1;
        }
        final_hashes.truncate(expected_hashes);

        BranchNodeCompact::new(
            TrieMask::new(state_mask),
            TrieMask::new(tree_mask),
            TrieMask::new(hash_mask),
            final_hashes,
            None,
        )
    }

    /// Helper function to create a simple test `MockTrieCursor`
    fn create_mock_cursor(trie_nodes: BTreeMap<Nibbles, BranchNodeCompact>) -> MockTrieCursor {
        let factory = MockTrieCursorFactory::new(trie_nodes, B256Map::default());
        factory.account_trie_cursor().unwrap()
    }

    fn verified_trie(factory: &MockHashedCursorFactory) -> MockTrieCursorFactory {
        let (_, updates) = crate::StateRoot::new(NoopTrieCursorFactory, factory.clone())
            .root_with_updates()
            .unwrap();
        let mut storages = updates
            .storage_tries
            .into_iter()
            .map(|(address, updates)| {
                (address, updates.storage_nodes.into_iter().collect::<BTreeMap<_, _>>())
            })
            .collect::<B256Map<_>>();
        let mut cursor = factory.hashed_account_cursor().unwrap();
        let mut next = cursor.seek(B256::ZERO).unwrap();
        while let Some((address, _)) = next {
            storages.entry(address).or_default();
            next = cursor.next().unwrap();
        }
        MockTrieCursorFactory::new(updates.account_nodes.into_iter().collect(), storages)
    }

    #[test]
    fn test_verifier_empty() {
        // Test with completely empty state
        let factory = MockHashedCursorFactory::new(BTreeMap::new(), B256Map::default());
        let trie = verified_trie(&factory);
        let mut verifier = Verifier::new(&trie, factory).unwrap();
        for output in verifier.by_ref() {
            assert_matches!(output.unwrap(), Output::Progress(_));
        }
        assert!(verifier.complete);
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_verifier_basic() {
        // Simple test with a few accounts and storage
        let mut accounts = BTreeMap::new();
        let mut storage_tries = B256Map::default();

        // Create test accounts
        let addr1 = keccak256(Address::with_last_byte(1));
        accounts.insert(
            addr1,
            Account {
                nonce: 1,
                balance: U256::from(1000),
                bytecode_hash: Some(keccak256(b"code1")),
                ..Default::default()
            },
        );

        // Add storage for the account
        let mut storage1 = BTreeMap::new();
        storage1.insert(keccak256(B256::with_last_byte(1)), U256::from(100));
        storage1.insert(keccak256(B256::with_last_byte(2)), U256::from(200));
        storage_tries.insert(addr1, storage1);

        let factory = MockHashedCursorFactory::new(accounts, storage_tries);
        let trie = verified_trie(&factory);
        let mut verifier = Verifier::new(&trie, factory).unwrap();
        for output in verifier.by_ref() {
            assert_matches!(output.unwrap(), Output::Progress(_));
        }
        assert!(verifier.complete);
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn test_verifier_multiple_accounts() {
        // Test with multiple accounts to verify ordering
        let mut accounts = BTreeMap::new();
        let mut storage_tries = B256Map::default();

        // Create multiple test addresses
        for i in 1u8..=3 {
            let addr = keccak256([i; 20]);
            accounts.insert(
                addr,
                Account {
                    nonce: i as u64,
                    balance: U256::from(i as u64 * 1000),
                    bytecode_hash: (i == 2).then(|| keccak256([i])),
                    ..Default::default()
                },
            );

            // Add some storage for each account
            let mut storage = BTreeMap::new();
            for j in 0..i {
                storage.insert(keccak256(B256::with_last_byte(j)), U256::from(j as u64 * 10));
            }
            if !storage.is_empty() {
                storage_tries.insert(addr, storage);
            }
        }

        let factory = MockHashedCursorFactory::new(accounts, storage_tries);
        let trie = verified_trie(&factory);
        let mut verifier = Verifier::new(&trie, factory).unwrap();
        for output in verifier.by_ref() {
            assert_matches!(output.unwrap(), Output::Progress(_));
        }
        assert!(verifier.complete);
    }

    #[test]
    fn test_single_verifier_new() {
        // Test creating a new SingleVerifier for account trie
        let trie_nodes = BTreeMap::from([(
            Nibbles::from_nibbles([0x1]),
            test_branch_node(0b1111, 0, 0, vec![]),
        )]);

        let cursor = create_mock_cursor(trie_nodes);
        let verifier = SingleVerifier::new(None, cursor).unwrap();

        // Should have seeked to the beginning and found the first node
        assert!(verifier.curr.is_some());
    }

    #[test]
    fn test_single_verifier_next_exact_match() {
        // Test when the expected node matches exactly
        let node1 = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);
        let node2 = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::repeat_byte(2u8)]);

        let trie_nodes = BTreeMap::from([
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x2]), node2),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // Call next with the exact node that exists
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();

        // Should have no outputs
        assert!(outputs.is_empty());
    }

    #[test]
    fn test_single_verifier_next_wrong_value() {
        // Test when the path matches but value is different
        let node_in_trie = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);
        let node_expected = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::repeat_byte(2u8)]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x1]), node_in_trie.clone())]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // Call next with different node value
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node_expected.clone()).unwrap();

        // Should have one "wrong" output
        assert_eq!(outputs.len(), 1);
        assert_matches!(
            &outputs[0],
            Output::AccountWrong { path, expected, found }
                if *path == Nibbles::from_nibbles([0x1]) && *expected == node_expected && *found == node_in_trie
        );
    }

    #[test]
    fn test_single_verifier_next_missing() {
        // Test when expected node doesn't exist in trie
        let node1 = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);
        let node_missing = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::repeat_byte(2u8)]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x3]), node1)]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // Call next with a node that comes before any in the trie
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node_missing.clone()).unwrap();

        // Should have one "missing" output
        assert_eq!(outputs.len(), 1);
        assert_matches!(
            &outputs[0],
            Output::AccountMissing(path, node)
                if *path == Nibbles::from_nibbles([0x1]) && *node == node_missing
        );
    }

    #[test]
    fn test_single_verifier_next_extra() {
        // Test when trie has extra nodes not in expected
        // Create a proper trie structure with root
        let node_root = test_branch_node(0b1110, 0, 0b1110, vec![]); // root has children at 1, 2, 3
        let node1 = test_branch_node(0b0001, 0, 0b0001, vec![]);
        let node2 = test_branch_node(0b0010, 0, 0b0010, vec![]);
        let node3 = test_branch_node(0b0100, 0, 0b0100, vec![]);

        let trie_nodes = BTreeMap::from([
            (Nibbles::new(), node_root.clone()),
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x2]), node2.clone()),
            (Nibbles::from_nibbles([0x3]), node3.clone()),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // The depth-first iterator produces in post-order: 0x1, 0x2, 0x3, root
        // We only provide 0x1 and 0x3, skipping 0x2 and root
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x3]), node3).unwrap();
        verifier.finalize(&mut outputs).unwrap();

        // Should have two "extra" outputs for nodes in the trie that we skipped
        if outputs.len() != 2 {
            eprintln!("Expected 2 outputs, got {}:", outputs.len());
            for inc in &outputs {
                eprintln!("  {:?}", inc);
            }
        }
        assert_eq!(outputs.len(), 2);
        assert_matches!(
            &outputs[0],
            Output::AccountExtra(path, node)
                if *path == Nibbles::from_nibbles([0x2]) && *node == node2
        );
        assert_matches!(
            &outputs[1],
            Output::AccountExtra(path, node)
                if *path == Nibbles::new() && *node == node_root
        );
    }

    #[test]
    fn test_single_verifier_finalize() {
        // Test finalize marks all remaining nodes as extra
        let node_root = test_branch_node(0b1110, 0, 0b1110, vec![]); // root has children at 1, 2, 3
        let node1 = test_branch_node(0b0001, 0, 0b0001, vec![]);
        let node2 = test_branch_node(0b0010, 0, 0b0010, vec![]);
        let node3 = test_branch_node(0b0100, 0, 0b0100, vec![]);

        let trie_nodes = BTreeMap::from([
            (Nibbles::new(), node_root.clone()),
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x2]), node2.clone()),
            (Nibbles::from_nibbles([0x3]), node3.clone()),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // The depth-first iterator produces in post-order: 0x1, 0x2, 0x3, root
        // Process first two nodes correctly
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x2]), node2).unwrap();
        assert!(outputs.is_empty());

        // Finalize - should mark remaining nodes (0x3 and root) as extra
        verifier.finalize(&mut outputs).unwrap();

        // Should have two extra nodes
        assert_eq!(outputs.len(), 2);
        assert_matches!(
            &outputs[0],
            Output::AccountExtra(path, node)
                if *path == Nibbles::from_nibbles([0x3]) && *node == node3
        );
        assert_matches!(
            &outputs[1],
            Output::AccountExtra(path, node)
                if *path == Nibbles::new() && *node == node_root
        );
    }

    #[test]
    fn test_single_verifier_storage_trie() {
        // Test SingleVerifier for storage trie (with account set)
        let account = B256::repeat_byte(42u8);
        let node = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);

        let trie_nodes = BTreeMap::from([(Nibbles::from_nibbles([0x1]), node)]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(Some(account), cursor).unwrap();
        let mut outputs = Vec::new();

        // Call next with missing node
        let missing_node = test_branch_node(0b0101, 0b0001, 0b0100, vec![B256::repeat_byte(2u8)]);
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x0]), missing_node.clone()).unwrap();

        // Should produce StorageMissing, not AccountMissing
        assert_eq!(outputs.len(), 1);
        assert_matches!(
            &outputs[0],
            Output::StorageMissing(acc, path, node)
                if *acc == account && *path == Nibbles::from_nibbles([0x0]) && *node == missing_node
        );
    }

    #[test]
    fn test_single_verifier_empty_trie() {
        // Test with empty trie cursor
        let trie_nodes = BTreeMap::new();
        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // Any node should be marked as missing
        let node = test_branch_node(0b1111, 0, 0b1111, vec![B256::repeat_byte(1u8)]);
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node.clone()).unwrap();

        assert_eq!(outputs.len(), 1);
        assert_matches!(
            &outputs[0],
            Output::AccountMissing(path, n)
                if *path == Nibbles::from_nibbles([0x1]) && *n == node
        );
    }

    #[test]
    fn test_single_verifier_depth_first_ordering() {
        // Test that nodes must be provided in depth-first order
        // Create nodes with proper parent-child relationships
        let node_root = test_branch_node(0b0110, 0, 0b0110, vec![]); // root has children at 1 and 2
        let node1 = test_branch_node(0b0110, 0, 0b0110, vec![]); // 0x1 has children at 1 and 2
        let node11 = test_branch_node(0b0001, 0, 0b0001, vec![]); // 0x11 is a leaf
        let node12 = test_branch_node(0b0010, 0, 0b0010, vec![]); // 0x12 is a leaf
        let node2 = test_branch_node(0b0100, 0, 0b0100, vec![]); // 0x2 is a leaf

        // The depth-first iterator will iterate from the root in this order:
        // root -> 0x1 -> 0x11, 0x12 (children of 0x1), then 0x2
        // But because of depth-first, we get: root, 0x1, 0x11, 0x12, 0x2
        let trie_nodes = BTreeMap::from([
            (Nibbles::new(), node_root.clone()),                 // root
            (Nibbles::from_nibbles([0x1]), node1.clone()),       // 0x1
            (Nibbles::from_nibbles([0x1, 0x1]), node11.clone()), // 0x11
            (Nibbles::from_nibbles([0x1, 0x2]), node12.clone()), // 0x12
            (Nibbles::from_nibbles([0x2]), node2.clone()),       // 0x2
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // The depth-first iterator produces nodes in post-order (children before parents)
        // Order: 0x11, 0x12, 0x1, 0x2, root
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x1]), node11).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x2]), node12).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x2]), node2).unwrap();
        verifier.next(&mut outputs, Nibbles::new(), node_root).unwrap();
        verifier.finalize(&mut outputs).unwrap();

        // All should match, no outputs
        if !outputs.is_empty() {
            eprintln!(
                "Test test_single_verifier_depth_first_ordering failed with {} outputs:",
                outputs.len()
            );
            for inc in &outputs {
                eprintln!("  {:?}", inc);
            }
        }
        assert!(outputs.is_empty());
    }

    #[test]
    fn test_single_verifier_wrong_depth_first_order() {
        // Test that providing nodes in wrong order produces outputs
        // Create a trie with parent-child relationship
        let node_root = test_branch_node(0b0010, 0, 0b0010, vec![]); // root has child at 1
        let node1 = test_branch_node(0b0010, 0, 0b0010, vec![]); // 0x1 has child at 1
        let node11 = test_branch_node(0b0001, 0, 0b0001, vec![]); // 0x11 is a leaf

        let trie_nodes = BTreeMap::from([
            (Nibbles::new(), node_root.clone()),
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x1, 0x1]), node11.clone()),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // Process in WRONG order (skip root, provide child before processing all nodes correctly)
        // The iterator will produce: root, 0x1, 0x11
        // But we provide: 0x11, root, 0x1 (completely wrong order)
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x1]), node11).unwrap();
        verifier.next(&mut outputs, Nibbles::new(), node_root).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();

        // Should have outputs since we provided them in wrong order
        assert!(!outputs.is_empty());
    }

    #[test]
    fn test_single_verifier_complex_depth_first() {
        // Test a complex tree structure with depth-first ordering
        // Build a tree structure with proper parent-child relationships
        let node_root = test_branch_node(0b0110, 0, 0b0110, vec![]); // root: children at nibbles 1
                                                                     // and 2
        let node1 = test_branch_node(0b0110, 0, 0b0110, vec![]); // 0x1: children at nibbles 1 and 2
        let node11 = test_branch_node(0b0110, 0, 0b0110, vec![]); // 0x11: children at nibbles 1 and
                                                                  // 2
        let node111 = test_branch_node(0b0001, 0, 0b0001, vec![]); // 0x111: leaf
        let node112 = test_branch_node(0b0010, 0, 0b0010, vec![]); // 0x112: leaf
        let node12 = test_branch_node(0b0100, 0, 0b0100, vec![]); // 0x12: leaf
        let node2 = test_branch_node(0b0010, 0, 0b0010, vec![]); // 0x2: child at nibble 1
        let node21 = test_branch_node(0b0001, 0, 0b0001, vec![]); // 0x21: leaf

        // Create the trie structure
        let trie_nodes = BTreeMap::from([
            (Nibbles::new(), node_root.clone()),
            (Nibbles::from_nibbles([0x1]), node1.clone()),
            (Nibbles::from_nibbles([0x1, 0x1]), node11.clone()),
            (Nibbles::from_nibbles([0x1, 0x1, 0x1]), node111.clone()),
            (Nibbles::from_nibbles([0x1, 0x1, 0x2]), node112.clone()),
            (Nibbles::from_nibbles([0x1, 0x2]), node12.clone()),
            (Nibbles::from_nibbles([0x2]), node2.clone()),
            (Nibbles::from_nibbles([0x2, 0x1]), node21.clone()),
        ]);

        let cursor = create_mock_cursor(trie_nodes);
        let mut verifier = SingleVerifier::new(None, cursor).unwrap();
        let mut outputs = Vec::new();

        // The depth-first iterator produces nodes in post-order (children before parents)
        // Order: 0x111, 0x112, 0x11, 0x12, 0x1, 0x21, 0x2, root
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x1, 0x1]), node111).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x1, 0x2]), node112).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x1]), node11).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1, 0x2]), node12).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x1]), node1).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x2, 0x1]), node21).unwrap();
        verifier.next(&mut outputs, Nibbles::from_nibbles([0x2]), node2).unwrap();
        verifier.next(&mut outputs, Nibbles::new(), node_root).unwrap();
        verifier.finalize(&mut outputs).unwrap();

        // All should match, no outputs
        if !outputs.is_empty() {
            eprintln!(
                "Test test_single_verifier_complex_depth_first failed with {} outputs:",
                outputs.len()
            );
            for inc in &outputs {
                eprintln!("  {:?}", inc);
            }
        }
        assert!(outputs.is_empty());
    }
}
