// Copyright 2019 Parity Technologies (UK) Ltd.
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

//! The `Entry` API for querying and modifying the entries of a `KBucketsTable`
//! representing the nodes participating in the Kademlia DHT.

use crate::{
    kbucket::{
        bucket::{InsertResult, KBucket, Node, NodeStatus},
        key::Key,
        ConnectionState, FailureReason, UpdateResult,
    },
    ConnectionDirection,
};

/// An immutable by-reference view of a bucket entry.
pub struct EntryRefView<'a, TPeerId, TVal: Eq> {
    /// The node represented by the entry.
    pub node: NodeRefView<'a, TPeerId, TVal>,
    /// The status of the node identified by the key.
    pub status: NodeStatus,
}

/// An immutable by-reference view of a `Node`.
pub struct NodeRefView<'a, TPeerId, TVal: Eq> {
    pub key: &'a Key<TPeerId>,
    pub value: &'a TVal,
}

/// A cloned, immutable view of an entry that is either present in a bucket
/// or pending insertion.
#[derive(Clone, Debug)]
pub struct EntryView<TPeerId, TVal: Eq> {
    /// The node represented by the entry.
    pub node: Node<TPeerId, TVal>,
    /// The status of the node.
    pub status: NodeStatus,
}

impl<TPeerId, TVal: Eq> AsRef<Key<TPeerId>> for EntryView<TPeerId, TVal> {
    fn as_ref(&self) -> &Key<TPeerId> {
        &self.node.key
    }
}

/// A reference into a single entry of a `KBucketsTable`.
#[derive(Debug)]
pub enum Entry<'a, TPeerId, TVal: Eq> {
    /// The entry is present in a bucket.
    Present(PresentEntry<'a, TPeerId, TVal>, NodeStatus),
    /// The entry is pending insertion in a bucket.
    Pending(PendingEntry<'a, TPeerId, TVal>, NodeStatus),
    /// The entry is absent and may be inserted.
    Absent(AbsentEntry<'a, TPeerId, TVal>),
    /// The entry represents the local node.
    SelfEntry,
}

/// The internal representation of the different states of an `Entry`,
/// referencing the associated key and bucket.
#[derive(Debug)]
struct EntryRef<'a, TPeerId, TVal: Eq> {
    bucket: &'a mut KBucket<TPeerId, TVal>,
    key: &'a Key<TPeerId>,
}

impl<'a, TPeerId, TVal> Entry<'a, TPeerId, TVal>
where
    TPeerId: Clone,
    TVal: Eq,
{
    /// Creates a new `Entry` for a `Key`, encapsulating access to a bucket.
    pub(super) fn new(bucket: &'a mut KBucket<TPeerId, TVal>, key: &'a Key<TPeerId>) -> Self {
        if let Some(pos) = bucket.position(key) {
            let status = bucket.status(pos);
            Entry::Present(PresentEntry::new(bucket, key), status)
        } else if let Some(pending) = bucket.as_pending(key) {
            let status = pending.status();
            Entry::Pending(PendingEntry::new(bucket, key), status)
        } else {
            Entry::Absent(AbsentEntry::new(bucket, key))
        }
    }
}

/// An entry present in a bucket.
#[derive(Debug)]
pub struct PresentEntry<'a, TPeerId, TVal: Eq>(EntryRef<'a, TPeerId, TVal>);

impl<'a, TPeerId, TVal> PresentEntry<'a, TPeerId, TVal>
where
    TPeerId: Clone,
    TVal: Eq,
{
    fn new(bucket: &'a mut KBucket<TPeerId, TVal>, key: &'a Key<TPeerId>) -> Self {
        PresentEntry(EntryRef { bucket, key })
    }

    /// Returns the value associated with the key.
    pub fn value(&self) -> &TVal {
        &self
            .0
            .bucket
            .get(self.0.key)
            .expect("We can only build a ConnectedEntry if the entry is in the bucket; QED")
            .value
    }

    /// Sets the status of the entry.
    /// This can fail if the new state violates buckets or table conditions.
    pub fn update(
        self,
        state: ConnectionState,
        direction: Option<ConnectionDirection>,
    ) -> Result<Self, FailureReason> {
        match self.0.bucket.update_status(self.0.key, state, direction) {
            UpdateResult::Failed(reason) => Err(reason),
            UpdateResult::UpdatedAndPromoted |
            UpdateResult::Updated |
            UpdateResult::UpdatedPending |
            UpdateResult::NotModified => {
                // Successful update, return the new entry
                Ok(Self::new(self.0.bucket, self.0.key))
            }
        }
    }

    /// Removes the entry from the table.
    pub fn remove(self) {
        self.0.bucket.remove(self.0.key);
    }
}

/// An entry waiting for a slot to be available in a bucket.
#[derive(Debug)]
pub struct PendingEntry<'a, TPeerId, TVal: Eq>(EntryRef<'a, TPeerId, TVal>);

impl<'a, TPeerId, TVal: Eq> PendingEntry<'a, TPeerId, TVal>
where
    TPeerId: Clone,
    TVal: Eq,
{
    fn new(bucket: &'a mut KBucket<TPeerId, TVal>, key: &'a Key<TPeerId>) -> Self {
        PendingEntry(EntryRef { bucket, key })
    }

    /// Returns the value associated with the key.
    pub fn value(&mut self) -> &mut TVal {
        self.0
            .bucket
            .pending_mut()
            .expect("We can only build a ConnectedPendingEntry if the entry is pending; QED")
            .value_mut()
    }

    /// Updates the status of the pending entry.
    pub fn update(self, status: NodeStatus) -> PendingEntry<'a, TPeerId, TVal> {
        self.0.bucket.update_pending(status);
        PendingEntry::new(self.0.bucket, self.0.key)
    }

    /// Removes the entry from the table.
    pub fn remove(self) {
        self.0.bucket.remove(self.0.key);
    }
}

/// An entry that is not present in any bucket.
#[derive(Debug)]
pub struct AbsentEntry<'a, TPeerId, TVal: Eq>(EntryRef<'a, TPeerId, TVal>);

impl<'a, TPeerId, TVal> AbsentEntry<'a, TPeerId, TVal>
where
    TPeerId: Clone,
    TVal: Eq,
{
    fn new(bucket: &'a mut KBucket<TPeerId, TVal>, key: &'a Key<TPeerId>) -> Self {
        AbsentEntry(EntryRef { bucket, key })
    }

    /// Attempts to insert the entry into a bucket.
    pub fn insert(self, value: TVal, status: NodeStatus) -> InsertResult<TPeerId> {
        self.0.bucket.insert(Node { key: self.0.key.clone(), value, status })
    }
}
