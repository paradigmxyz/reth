// Copyright 2018 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

// This basis of this file has been taken from the discv5 codebase: <https://github.com/emhane/discv5/> which is adapted from the rust-libp2p codebase: https://github.com/libp2p/rust-libp2p

//! Implementation of a Kademlia routing table as used by a single peer
//! participating in a Kademlia DHT.
//!
//! The entry point for the API of this module is a [`KBucketsTable`].
//!
//! ## Pending Insertions
//!
//! When the bucket associated with the `Key` of an inserted entry is full
//! but contains disconnected nodes, it accepts a [`PendingEntry`].
//! Pending entries are inserted lazily when their timeout is found to be expired
//! upon querying the `KBucketsTable`. When that happens, the `KBucketsTable` records
//! an [`AppliedPending`] result which must be consumed by calling [`take_applied_pending`]
//! regularly and / or after performing lookup operations like [`entry`] and [`closest_keys`].
//!
//! [`entry`]: KBucketsTable::entry
//! [`closest_keys`]: KBucketsTable::closest_keys
//! [`take_applied_pending`]: KBucketsTable::take_applied_pending

// [Implementation Notes]
//
// 1. Routing Table Layout
//
// The routing table is currently implemented as a fixed-size "array" of
// buckets, ordered by increasing distance relative to a local key
// that identifies the local peer. This is an often-used, simplified
// implementation that approximates the properties of the b-tree (or prefix tree)
// implementation described in the full paper [0], whereby buckets are split on-demand.
// This should be treated as an implementation detail, however, so that the
// implementation may change in the future without breaking the API.
//
// 2. Replacement Cache
//
// In this implementation, the "replacement cache" for unresponsive peers
// consists of a single entry per bucket. Furthermore, this implementation is
// currently tailored to connection-oriented transports, meaning that the
// "LRU"-based ordering of entries in a bucket is actually based on the last reported
// connection status of the corresponding peers, from least-recently (dis)connected to
// most-recently (dis)connected, and controlled through the `Entry` API. As a result,
// the nodes in the buckets are not reordered as a result of RPC activity, but only as a
// result of nodes being marked as connected or disconnected. In particular,
// if a bucket is full and contains only entries for peers that are considered
// connected, no pending entry is accepted. See the `bucket` submodule for
// further details.
//
// [0]: https://pdos.csail.mit.edu/~petar/papers/maymounkov-kademlia-lncs.pdf

use crate::{
    kbucket::{
        bucket::{AppliedPending, Node, NodeStatus},
        entry::{Entry, EntryRefView, NodeRefView},
        key::{Distance, Key},
    },
    ConnectionDirection,
};
use arrayvec::ArrayVec;
use bucket::KBucket;
pub use bucket::{
    ConnectionState, FailureReason, InsertResult as BucketInsertResult, UpdateResult,
    MAX_NODES_PER_BUCKET,
};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

mod bucket;
mod entry;
mod key;

/// Maximum number of k-buckets.
const NUM_BUCKETS: usize = 256;

/// Closest Iterator Output Value
pub struct ClosestValue<TNodeId, TVal> {
    pub key: Key<TNodeId>,
    pub value: TVal,
}

impl<TNodeId, TVal> AsRef<Key<TNodeId>> for ClosestValue<TNodeId, TVal> {
    fn as_ref(&self) -> &Key<TNodeId> {
        &self.key
    }
}

/// A key that can be returned from the `closest_keys` function, which indicates if the key matches
/// the predicate or not.
pub struct PredicateKey<TNodeId: Clone> {
    pub key: Key<TNodeId>,
    pub predicate_match: bool,
}

impl<TNodeId: Clone> From<PredicateKey<TNodeId>> for Key<TNodeId> {
    fn from(key: PredicateKey<TNodeId>) -> Self {
        key.key
    }
}

impl<TNodeId: Clone, TVal> From<PredicateValue<TNodeId, TVal>> for PredicateKey<TNodeId> {
    fn from(value: PredicateValue<TNodeId, TVal>) -> Self {
        PredicateKey { key: value.key, predicate_match: value.predicate_match }
    }
}

/// A value being returned from a predicate closest iterator.
pub struct PredicateValue<TNodeId: Clone, TVal> {
    pub key: Key<TNodeId>,
    pub predicate_match: bool,
    pub value: TVal,
}

impl<TNodeId: Clone, TVal> AsRef<Key<TNodeId>> for PredicateValue<TNodeId, TVal> {
    fn as_ref(&self) -> &Key<TNodeId> {
        &self.key
    }
}

impl<TNodeId: Clone, TVal> From<PredicateValue<TNodeId, TVal>> for Key<TNodeId> {
    fn from(key: PredicateValue<TNodeId, TVal>) -> Self {
        key.key
    }
}

/// A `KBucketsTable` represents a Kademlia routing table.
#[derive(Clone)]
pub struct KBucketsTable<TNodeId, TVal: Eq> {
    /// The key identifying the local peer that owns the routing table.
    local_key: Key<TNodeId>,
    /// The buckets comprising the routing table.
    buckets: Vec<KBucket<TNodeId, TVal>>,
    /// The list of evicted entries that have been replaced with pending
    /// entries since the last call to [`KBucketsTable::take_applied_pending`].
    applied_pending: VecDeque<AppliedPending<TNodeId, TVal>>,
}

#[must_use]
#[derive(Debug, Clone)]
/// Informs if the record was inserted.
pub enum InsertResult<TNodeId> {
    /// The node didn't exist and the new record was inserted.
    Inserted,
    /// The node was inserted into a pending state.
    Pending {
        /// The key of the least-recently connected entry that is currently considered
        /// disconnected and whose corresponding peer should be checked for connectivity
        /// in order to prevent it from being evicted. If connectivity to the peer is
        /// re-established, the corresponding entry should be updated with
        /// [`bucket::ConnectionState::Connected`].
        disconnected: Key<TNodeId>,
    },
    /// The node existed and the status was updated.
    StatusUpdated {
        // Returns true if the status updated promoted a disconnected node to a connected node.
        promoted_to_connected: bool,
    },
    /// The node existed and the value was updated.
    ValueUpdated,
    /// Both the status and value were updated.
    Updated {
        // Returns true if the status updated promoted a disconnected node to a connected node.
        promoted_to_connected: bool,
    },
    /// The pending slot was updated.
    UpdatedPending,
    /// The record failed to be inserted. This can happen to not passing table/bucket filters or
    /// the bucket was full.
    Failed(FailureReason),
}

/// A (type-safe) index into a `KBucketsTable`, i.e. a non-negative integer in the
/// interval `[0, NUM_BUCKETS)`.
#[derive(Copy, Clone)]
struct BucketIndex(usize);

impl BucketIndex {
    /// Creates a new `BucketIndex` for a `Distance`.
    ///
    /// The given distance is interpreted as the distance from a `local_key` of
    /// a `KBucketsTable`. If the distance is zero, `None` is returned, in
    /// recognition of the fact that the only key with distance `0` to a
    /// `local_key` is the `local_key` itself, which does not belong in any
    /// bucket.
    fn new(d: &Distance) -> Option<BucketIndex> {
        (NUM_BUCKETS - d.0.leading_zeros() as usize).checked_sub(1).map(BucketIndex)
    }

    /// Gets the index value as an unsigned integer.
    fn get(self) -> usize {
        self.0
    }
}

impl<TNodeId, TVal> KBucketsTable<TNodeId, TVal>
where
    TNodeId: Clone,
    TVal: Eq,
{
    /// Creates a new, empty Kademlia routing table with entries partitioned
    /// into buckets as per the Kademlia protocol.
    ///
    /// The given `pending_timeout` specifies the duration after creation of
    /// a [`PendingEntry`] after which it becomes eligible for insertion into
    /// a full bucket, replacing the least-recently (dis)connected node.
    ///
    /// A filter can be applied that limits entries into a bucket based on the buckets contents.
    /// Entries that fail the filter, will not be inserted.
    pub fn new(
        local_key: Key<TNodeId>,
        pending_timeout: Duration,
        max_incoming_per_bucket: usize,
    ) -> Self {
        KBucketsTable {
            local_key,
            buckets: (0..NUM_BUCKETS)
                .map(|_| KBucket::new(pending_timeout, max_incoming_per_bucket))
                .collect(),
            applied_pending: VecDeque::new(),
        }
    }

    // Updates a node's status if it exists in the table.
    // This checks all table and bucket filters before performing the update.
    pub fn update_node_status(
        &mut self,
        key: &Key<TNodeId>,
        state: ConnectionState,
        direction: Option<ConnectionDirection>,
    ) -> UpdateResult {
        let index = BucketIndex::new(&self.local_key.distance(key));
        if let Some(i) = index {
            let bucket = &mut self.buckets[i.get()];
            if let Some(applied) = bucket.apply_pending() {
                self.applied_pending.push_back(applied)
            }

            bucket.update_status(key, state, direction)
        } else {
            UpdateResult::NotModified // The key refers to our current node.
        }
    }

    /// Updates a node's value if it exists in the table.
    ///
    /// Optionally the connection state can be modified.
    pub fn update_node(
        &mut self,
        key: &Key<TNodeId>,
        value: TVal,
        state: Option<ConnectionState>,
    ) -> UpdateResult {
        // Apply the table filter

        let index = BucketIndex::new(&self.local_key.distance(key));
        if let Some(i) = index {
            let bucket = &mut self.buckets[i.get()];
            if let Some(applied) = bucket.apply_pending() {
                self.applied_pending.push_back(applied)
            }

            let update_result = bucket.update_value(key, value);

            if let UpdateResult::Failed(_) = &update_result {
                return update_result
            }

            // If we need to update the connection state, update it here.
            let status_result = if let Some(state) = state {
                bucket.update_status(key, state, None)
            } else {
                UpdateResult::NotModified
            };

            // Return an appropriate value
            match (&update_result, &status_result) {
                (_, UpdateResult::Failed(_)) => status_result,
                (UpdateResult::Failed(_), _) => update_result,
                (_, UpdateResult::UpdatedAndPromoted) => UpdateResult::UpdatedAndPromoted,
                (UpdateResult::UpdatedPending, _) => UpdateResult::UpdatedPending,
                (_, UpdateResult::UpdatedPending) => UpdateResult::UpdatedPending,
                (UpdateResult::NotModified, UpdateResult::NotModified) => UpdateResult::NotModified,
                (_, _) => UpdateResult::Updated,
            }
        } else {
            UpdateResult::NotModified // The key refers to our current node.
        }
    }

    // Attempts to insert or update
    pub fn insert_or_update(
        &mut self,
        key: &Key<TNodeId>,
        value: TVal,
        status: NodeStatus,
    ) -> InsertResult<TNodeId> {
        // Check the table filter
        let index = BucketIndex::new(&self.local_key.distance(key));
        if let Some(i) = index {
            let bucket = &mut self.buckets[i.get()];
            if let Some(applied) = bucket.apply_pending() {
                self.applied_pending.push_back(applied)
            }

            // If the node doesn't exist, insert it
            if bucket.position(key).is_none() {
                let node = Node { key: key.clone(), value, status };
                match bucket.insert(node) {
                    bucket::InsertResult::NodeExists => unreachable!("Node must exist"),
                    bucket::InsertResult::Full => InsertResult::Failed(FailureReason::BucketFull),
                    bucket::InsertResult::TooManyIncoming => {
                        InsertResult::Failed(FailureReason::TooManyIncoming)
                    }
                    bucket::InsertResult::Pending { disconnected } => {
                        InsertResult::Pending { disconnected }
                    }
                    bucket::InsertResult::Inserted => InsertResult::Inserted,
                }
            } else {
                // The node exists in the bucket
                // Attempt to update the status
                let update_status = bucket.update_status(key, status.state, Some(status.direction));

                if update_status.failed() {
                    // The node was removed from the table
                    return InsertResult::Failed(FailureReason::TooManyIncoming)
                }
                // Attempt to update the value
                let update_value = bucket.update_value(key, value);

                match (update_value, update_status) {
                    (UpdateResult::Updated { .. }, UpdateResult::Updated) => {
                        InsertResult::Updated { promoted_to_connected: false }
                    }
                    (UpdateResult::Updated { .. }, UpdateResult::UpdatedAndPromoted) => {
                        InsertResult::Updated { promoted_to_connected: true }
                    }
                    (UpdateResult::Updated { .. }, UpdateResult::NotModified) |
                    (UpdateResult::Updated { .. }, UpdateResult::UpdatedPending) => {
                        InsertResult::ValueUpdated
                    }
                    (UpdateResult::NotModified, UpdateResult::Updated) => {
                        InsertResult::StatusUpdated { promoted_to_connected: false }
                    }
                    (UpdateResult::NotModified, UpdateResult::UpdatedAndPromoted) => {
                        InsertResult::StatusUpdated { promoted_to_connected: true }
                    }
                    (UpdateResult::NotModified, UpdateResult::NotModified) => {
                        InsertResult::Updated { promoted_to_connected: false }
                    }
                    (UpdateResult::UpdatedPending, _) | (_, UpdateResult::UpdatedPending) => {
                        InsertResult::UpdatedPending
                    }
                    (UpdateResult::Failed(reason), _) => InsertResult::Failed(reason),
                    (_, UpdateResult::Failed(_)) => unreachable!("Status failure handled earlier."),
                    (UpdateResult::UpdatedAndPromoted, _) => {
                        unreachable!("Value update cannot promote a connection.")
                    }
                }
            }
        } else {
            // Cannot insert our local entry.
            InsertResult::Failed(FailureReason::InvalidSelfUpdate)
        }
    }

    /// Removes a node from the routing table. Returns `true` of the node existed.
    pub fn remove(&mut self, key: &Key<TNodeId>) -> bool {
        let index = BucketIndex::new(&self.local_key.distance(key));
        if let Some(i) = index {
            let bucket = &mut self.buckets[i.get()];
            if let Some(applied) = bucket.apply_pending() {
                self.applied_pending.push_back(applied)
            }
            bucket.remove(key)
        } else {
            false
        }
    }

    /// Returns an `Entry` for the given key, representing the state of the entry
    /// in the routing table.
    /// NOTE: This must be used with caution. Modifying values manually can bypass the internal
    /// table filters and ingoing/outgoing limits.
    pub fn entry<'a>(&'a mut self, key: &'a Key<TNodeId>) -> Entry<'a, TNodeId, TVal> {
        let index = BucketIndex::new(&self.local_key.distance(key));
        if let Some(i) = index {
            let bucket = &mut self.buckets[i.get()];
            if let Some(applied) = bucket.apply_pending() {
                self.applied_pending.push_back(applied)
            }
            Entry::new(bucket, key)
        } else {
            Entry::SelfEntry
        }
    }

    /// Returns an iterator over all the entries in the routing table.
    pub fn iter(&mut self) -> impl Iterator<Item = EntryRefView<'_, TNodeId, TVal>> {
        let applied_pending = &mut self.applied_pending;
        self.buckets.iter_mut().flat_map(move |table| {
            if let Some(applied) = table.apply_pending() {
                applied_pending.push_back(applied)
            }
            table.iter().map(move |n| EntryRefView {
                node: NodeRefView { key: &n.key, value: &n.value },
                status: n.status,
            })
        })
    }

    /// Returns an iterator over all the buckets in the routing table
    pub fn buckets_iter(&self) -> impl Iterator<Item = &KBucket<TNodeId, TVal>> {
        self.buckets.iter()
    }

    /// Returns an iterator over all the entries in the routing table to give to a table filter.
    ///
    /// This differs from the regular iterator as it doesn't take ownership of self and doesn't try
    /// to apply any pending nodes.
    fn table_iter(&self) -> impl Iterator<Item = &TVal> {
        self.buckets.iter().flat_map(move |table| table.iter().map(|n| &n.value))
    }

    /// Returns an iterator over all the entries in the routing table.
    /// Does not add pending node to kbucket to get an iterator which
    /// takes a reference instead of a mutable reference.
    pub fn iter_ref(&self) -> impl Iterator<Item = EntryRefView<'_, TNodeId, TVal>> {
        self.buckets.iter().flat_map(move |table| {
            table.iter().map(move |n| EntryRefView {
                node: NodeRefView { key: &n.key, value: &n.value },
                status: n.status,
            })
        })
    }

    /// Consumes the next applied pending entry, if any.
    ///
    /// When an entry is attempted to be inserted and the respective bucket is full,
    /// it may be recorded as pending insertion after a timeout, see [`InsertResult::Pending`].
    ///
    /// If the oldest currently disconnected entry in the respective bucket does not change
    /// its status until the timeout of pending entry expires, it is evicted and
    /// the pending entry inserted instead. These insertions of pending entries
    /// happens lazily, whenever the `KBucketsTable` is accessed, and the corresponding
    /// buckets are updated accordingly. The fact that a pending entry was applied is
    /// recorded in the `KBucketsTable` in the form of `AppliedPending` results, which must be
    /// consumed by calling this function.
    pub fn take_applied_pending(&mut self) -> Option<AppliedPending<TNodeId, TVal>> {
        self.applied_pending.pop_front()
    }

    /// Returns an iterator over the keys that are contained in a kbucket, specified by a log2
    /// distance.
    pub fn nodes_by_distances(
        &mut self,
        log2_distances: &[u64],
        max_nodes: usize,
    ) -> Vec<EntryRefView<'_, TNodeId, TVal>> {
        // Filter log2 distances to only include those in the closed interval [1, 256]
        let distances = log2_distances
            .iter()
            .filter_map(|&d| if d > 0 && d <= (NUM_BUCKETS as u64) { Some(d) } else { None })
            .collect::<Vec<_>>();

        // Apply pending nodes
        for distance in &distances {
            // The log2 distance ranges from 1-256 and is always 1 more than the bucket index. For
            // this reason we subtract 1 from log2 distance to get the correct bucket
            // index.
            let bucket = &mut self.buckets[(distance - 1) as usize];
            if let Some(applied) = bucket.apply_pending() {
                self.applied_pending.push_back(applied)
            }
        }

        // Find the matching nodes
        let mut matching_nodes = Vec::new();

        // Note we search via distance in order
        for distance in distances {
            let bucket = &self.buckets[(distance - 1) as usize];
            for node in bucket.iter().map(|n| {
                let node = NodeRefView { key: &n.key, value: &n.value };
                EntryRefView { node, status: n.status }
            }) {
                matching_nodes.push(node);
                // Exit early if we have found enough nodes
                if matching_nodes.len() >= max_nodes {
                    return matching_nodes
                }
            }
        }
        matching_nodes
    }

    /// Returns an iterator over the keys closest to `target`, ordered by
    /// increasing distance.
    pub fn closest_keys<'a, T>(
        &'a mut self,
        target: &'a Key<T>,
    ) -> impl Iterator<Item = Key<TNodeId>> + 'a
    where
        T: Clone,
    {
        let distance = self.local_key.distance(target);
        ClosestIter {
            target,
            iter: None,
            table: self,
            buckets_iter: ClosestBucketsIter::new(distance),
            fmap: |b: &KBucket<TNodeId, TVal>| -> ArrayVec<_, MAX_NODES_PER_BUCKET> {
                b.iter().map(|n| n.key.clone()).collect()
            },
        }
    }

    /// Returns an iterator over the keys closest to `target`, ordered by
    /// increasing distance.
    pub fn closest_values<'a, T>(
        &'a mut self,
        target: &'a Key<T>,
    ) -> impl Iterator<Item = ClosestValue<TNodeId, TVal>> + 'a
    where
        T: Clone,
        TVal: Clone,
    {
        let distance = self.local_key.distance(target);
        ClosestIter {
            target,
            iter: None,
            table: self,
            buckets_iter: ClosestBucketsIter::new(distance),
            fmap: |b: &KBucket<TNodeId, TVal>| -> ArrayVec<_, MAX_NODES_PER_BUCKET> {
                b.iter()
                    .map(|n| ClosestValue { key: n.key.clone(), value: n.value.clone() })
                    .collect()
            },
        }
    }

    /// Returns an iterator over the keys closest to `target`, ordered by
    /// increasing distance specifying which keys agree with a value predicate.
    pub fn closest_values_predicate<'a, T, F>(
        &'a mut self,
        target: &'a Key<T>,
        predicate: F,
    ) -> impl Iterator<Item = PredicateValue<TNodeId, TVal>> + 'a
    where
        T: Clone,
        F: Fn(&TVal) -> bool + 'a,
        TVal: Clone,
    {
        let distance = self.local_key.distance(target);
        ClosestIter {
            target,
            iter: None,
            table: self,
            buckets_iter: ClosestBucketsIter::new(distance),
            fmap: move |b: &KBucket<TNodeId, TVal>| -> ArrayVec<_, MAX_NODES_PER_BUCKET> {
                b.iter()
                    .map(|n| PredicateValue {
                        key: n.key.clone(),
                        predicate_match: predicate(&n.value),
                        value: n.value.clone(),
                    })
                    .collect()
            },
        }
    }

    /// Returns a reference to a bucket given the key. Returns None if bucket does not exist.
    pub fn get_bucket<'a>(&'a self, key: &Key<TNodeId>) -> Option<&'a KBucket<TNodeId, TVal>> {
        let index = BucketIndex::new(&self.local_key.distance(key));
        if let Some(i) = index {
            let bucket = &self.buckets[i.get()];
            Some(bucket)
        } else {
            None
        }
    }

    /// Returns a bucket index given the key. Returns None if bucket index does not exist.
    pub fn get_index(&self, key: &Key<TNodeId>) -> Option<usize> {
        let index = BucketIndex::new(&self.local_key.distance(key));
        index.map(|i| i.get())
    }
}

/// An iterator over (some projection of) the closest entries in a
/// `KBucketsTable` w.r.t. some target `Key`.
struct ClosestIter<'a, TTarget, TNodeId, TVal: Eq, TMap, TOut> {
    /// A reference to the target key whose distance to the local key determines
    /// the order in which the buckets are traversed. The resulting
    /// array from projecting the entries of each bucket using `fmap` is
    /// sorted according to the distance to the target.
    target: &'a Key<TTarget>,
    /// A reference to all buckets of the `KBucketsTable`.
    table: &'a mut KBucketsTable<TNodeId, TVal>,
    /// The iterator over the bucket indices in the order determined by the
    /// distance of the local key to the target.
    buckets_iter: ClosestBucketsIter,
    /// The iterator over the entries in the currently traversed bucket.
    iter: Option<arrayvec::IntoIter<TOut, MAX_NODES_PER_BUCKET>>,
    /// The projection function / mapping applied on each bucket as
    /// it is encountered, producing the next `iter`ator.
    fmap: TMap,
}

/// An iterator over the bucket indices, in the order determined by the `Distance` of
/// a target from the `local_key`, such that the entries in the buckets are incrementally
/// further away from the target, starting with the bucket covering the target.
struct ClosestBucketsIter {
    /// The distance to the `local_key`.
    distance: Distance,
    /// The current state of the iterator.
    state: ClosestBucketsIterState,
}

/// Operating states of a `ClosestBucketsIter`.
enum ClosestBucketsIterState {
    /// The starting state of the iterator yields the first bucket index and
    /// then transitions to `ZoomIn`.
    Start(BucketIndex),
    /// The iterator "zooms in" to to yield the next bucket containing nodes that
    /// are incrementally closer to the local node but further from the `target`.
    /// These buckets are identified by a `1` in the corresponding bit position
    /// of the distance bit string. When bucket `0` is reached, the iterator
    /// transitions to `ZoomOut`.
    ZoomIn(BucketIndex),
    /// Once bucket `0` has been reached, the iterator starts "zooming out"
    /// to buckets containing nodes that are incrementally further away from
    /// both the local key and the target. These are identified by a `0` in
    /// the corresponding bit position of the distance bit string. When bucket
    /// `255` is reached, the iterator transitions to state `Done`.
    ZoomOut(BucketIndex),
    /// The iterator is in this state once it has visited all buckets.
    Done,
}

impl ClosestBucketsIter {
    fn new(distance: Distance) -> Self {
        let state = match BucketIndex::new(&distance) {
            Some(i) => ClosestBucketsIterState::Start(i),
            None => ClosestBucketsIterState::Done,
        };
        Self { distance, state }
    }

    fn next_in(&self, i: BucketIndex) -> Option<BucketIndex> {
        (0..i.get()).rev().find_map(|i| {
            if self.distance.0.bit(i) {
                Some(BucketIndex(i))
            } else {
                None
            }
        })
    }

    fn next_out(&self, i: BucketIndex) -> Option<BucketIndex> {
        (i.get() + 1..NUM_BUCKETS).find_map(|i| {
            if !self.distance.0.bit(i) {
                Some(BucketIndex(i))
            } else {
                None
            }
        })
    }
}

impl Iterator for ClosestBucketsIter {
    type Item = BucketIndex;

    fn next(&mut self) -> Option<Self::Item> {
        match self.state {
            ClosestBucketsIterState::Start(i) => {
                self.state = ClosestBucketsIterState::ZoomIn(i);
                Some(i)
            }
            ClosestBucketsIterState::ZoomIn(i) => {
                if let Some(i) = self.next_in(i) {
                    self.state = ClosestBucketsIterState::ZoomIn(i);
                    Some(i)
                } else {
                    let i = BucketIndex(0);
                    self.state = ClosestBucketsIterState::ZoomOut(i);
                    Some(i)
                }
            }
            ClosestBucketsIterState::ZoomOut(i) => {
                if let Some(i) = self.next_out(i) {
                    self.state = ClosestBucketsIterState::ZoomOut(i);
                    Some(i)
                } else {
                    self.state = ClosestBucketsIterState::Done;
                    None
                }
            }
            ClosestBucketsIterState::Done => None,
        }
    }
}

impl<TTarget, TNodeId, TVal, TMap, TOut> Iterator
    for ClosestIter<'_, TTarget, TNodeId, TVal, TMap, TOut>
where
    TNodeId: Clone,
    TVal: Eq,
    TMap: Fn(&KBucket<TNodeId, TVal>) -> ArrayVec<TOut, MAX_NODES_PER_BUCKET>,
    TOut: AsRef<Key<TNodeId>>,
{
    type Item = TOut;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match &mut self.iter {
                Some(iter) => match iter.next() {
                    Some(k) => return Some(k),
                    None => self.iter = None,
                },
                None => {
                    if let Some(i) = self.buckets_iter.next() {
                        let bucket = &mut self.table.buckets[i.get()];
                        if let Some(applied) = bucket.apply_pending() {
                            self.table.applied_pending.push_back(applied)
                        }
                        let mut v = (self.fmap)(bucket);
                        v.sort_by(|a, b| {
                            self.target.distance(a.as_ref()).cmp(&self.target.distance(b.as_ref()))
                        });
                        self.iter = Some(v.into_iter());
                    } else {
                        return None
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{bucket::InsertResult as BucketInsertResult, *};
    use crate::NodeId;

    fn connected_state() -> NodeStatus {
        NodeStatus { state: ConnectionState::Connected, direction: ConnectionDirection::Outgoing }
    }

    fn disconnected_state() -> NodeStatus {
        NodeStatus {
            state: ConnectionState::Disconnected,
            direction: ConnectionDirection::Outgoing,
        }
    }

    #[test]
    fn basic_closest() {
        let local_key = Key::from(NodeId::random());
        let other_id = Key::from(NodeId::random());

        let mut table =
            KBucketsTable::<_, ()>::new(local_key, Duration::from_secs(5), MAX_NODES_PER_BUCKET);
        if let Entry::Absent(entry) = table.entry(&other_id) {
            match entry.insert((), connected_state()) {
                BucketInsertResult::Inserted => (),
                _ => panic!(),
            }
        } else {
            panic!()
        }

        let res = table.closest_keys(&other_id).collect::<Vec<_>>();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0], other_id);
    }

    #[test]
    fn update_local_id_fails() {
        let local_key = Key::from(NodeId::random());
        let mut table = KBucketsTable::<_, ()>::new(
            local_key.clone(),
            Duration::from_secs(5),
            MAX_NODES_PER_BUCKET,
        );
        match table.entry(&local_key) {
            Entry::SelfEntry => (),
            _ => panic!(),
        }
    }

    #[test]
    fn closest() {
        let local_key = Key::from(NodeId::random());
        let mut table =
            KBucketsTable::<_, ()>::new(local_key, Duration::from_secs(5), MAX_NODES_PER_BUCKET);
        let mut count = 0;
        loop {
            if count == 100 {
                break
            }
            let key = Key::from(NodeId::random());
            if let Entry::Absent(e) = table.entry(&key) {
                match e.insert((), connected_state()) {
                    BucketInsertResult::Inserted => count += 1,
                    _ => continue,
                }
            } else {
                panic!("entry exists")
            }
        }

        let mut expected_keys: Vec<_> =
            table.buckets.iter().flat_map(|t| t.iter().map(|n| n.key.clone())).collect();

        for _ in 0..10 {
            let target_key = Key::from(NodeId::random());
            let keys = table.closest_keys(&target_key).collect::<Vec<_>>();
            // The list of keys is expected to match the result of a full-table scan.
            expected_keys.sort_by_key(|k| k.distance(&target_key));
            assert_eq!(keys, expected_keys);
        }
    }

    #[test]
    fn applied_pending() {
        let local_key = Key::from(NodeId::random());
        let mut table = KBucketsTable::<_, ()>::new(
            local_key.clone(),
            Duration::from_millis(1),
            MAX_NODES_PER_BUCKET,
        );
        let expected_applied;
        let full_bucket_index;
        loop {
            let key = Key::from(NodeId::random());
            if let Entry::Absent(e) = table.entry(&key) {
                match e.insert((), disconnected_state()) {
                    BucketInsertResult::Full => {
                        if let Entry::Absent(e) = table.entry(&key) {
                            match e.insert((), connected_state()) {
                                BucketInsertResult::Pending { disconnected } => {
                                    expected_applied = AppliedPending {
                                        inserted: key.clone(),
                                        evicted: Some(Node {
                                            key: disconnected,
                                            value: (),
                                            status: disconnected_state(),
                                        }),
                                    };
                                    full_bucket_index = BucketIndex::new(&key.distance(&local_key));
                                    break
                                }
                                _ => panic!(),
                            }
                        } else {
                            panic!()
                        }
                    }
                    _ => continue,
                }
            } else {
                panic!("entry exists")
            }
        }

        // Expire the timeout for the pending entry on the full bucket.`
        let full_bucket = &mut table.buckets[full_bucket_index.unwrap().get()];
        let elapsed = Instant::now() - Duration::from_secs(1);
        full_bucket.pending_mut().unwrap().set_ready_at(elapsed);

        match table.entry(&expected_applied.inserted) {
            Entry::Present(
                _,
                NodeStatus { state: ConnectionState::Connected, direction: _direction },
            ) => {}
            x => panic!("Unexpected entry: {:?}", x),
        }

        match table.entry(&expected_applied.evicted.as_ref().unwrap().key) {
            Entry::Absent(_) => {}
            x => panic!("Unexpected entry: {:?}", x),
        }

        assert_eq!(Some(expected_applied), table.take_applied_pending());
        assert_eq!(None, table.take_applied_pending());
    }
}
