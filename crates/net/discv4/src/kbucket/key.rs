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

use crate::NodeId;
use reth_primitives::{keccak256, U256};

/// A `Key` is a cryptographic hash, identifying both the nodes participating in
/// the Kademlia DHT, and records stored in the DHT.
///
/// The set of all `Key`s defines the Kademlia keyspace.
///
/// `Key`s have an XOR metric as defined in the Kademlia paper, i.e. the bitwise XOR of
/// the hash digests, interpreted as an integer. See [`Key::distance`].
///
/// A `Key` preserves the preimage of type `T` of the hash function. See [`Key::preimage`].
#[derive(Clone, Debug)]
pub struct Key<T> {
    preimage: T,
    hash: [u8; 32],
}

impl<T> PartialEq for Key<T> {
    fn eq(&self, other: &Key<T>) -> bool {
        self.hash == other.hash
    }
}

impl<T> Eq for Key<T> {}

impl<TPeerId> AsRef<Key<TPeerId>> for Key<TPeerId> {
    fn as_ref(&self) -> &Key<TPeerId> {
        self
    }
}

impl<T> Key<T> {
    /// Borrows the preimage of the key.
    pub fn preimage(&self) -> &T {
        &self.preimage
    }

    /// Converts the key into its preimage.
    pub fn into_preimage(self) -> T {
        self.preimage
    }

    /// Computes the distance of the keys according to the XOR metric.
    pub fn distance<U>(&self, other: &Key<U>) -> Distance {
        let a = U256::from(self.hash.as_slice());
        let b = U256::from(other.hash.as_slice());
        Distance(a ^ b)
    }

    // Used in the FINDNODE query outside of the k-bucket implementation.
    /// Computes the integer log-2 distance between two keys, assuming a 256-bit
    /// key. The output returns None if the key's are identical. The range is 1-256.
    pub fn log2_distance<U>(&self, other: &Key<U>) -> Option<u64> {
        let xor_dist = self.distance(other);
        let log_dist = u64::from(256 - xor_dist.0.leading_zeros());
        if log_dist == 0 {
            None
        } else {
            Some(log_dist)
        }
    }
}

impl From<NodeId> for Key<NodeId> {
    fn from(node_id: NodeId) -> Self {
        let hash = keccak256(node_id.as_ref());
        Key { preimage: node_id, hash: hash.0 }
    }
}

/// A distance between two `Key`s.
#[derive(Copy, Clone, PartialEq, Eq, Default, PartialOrd, Ord, Debug)]
pub struct Distance(pub(super) U256);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kbucket::bucket::tests::arbitrary_node_id;
    use quickcheck::*;

    impl Arbitrary for Key<NodeId> {
        fn arbitrary(g: &mut Gen) -> Key<NodeId> {
            Key::from(arbitrary_node_id(g))
        }
    }

    #[test]
    fn identity() {
        fn prop(a: Key<NodeId>) -> bool {
            a.distance(&a) == Distance::default()
        }
        quickcheck(prop as fn(_) -> _)
    }

    #[test]
    fn symmetry() {
        fn prop(a: Key<NodeId>, b: Key<NodeId>) -> bool {
            a.distance(&b) == b.distance(&a)
        }
        quickcheck(prop as fn(_, _) -> _)
    }

    #[test]
    fn triangle_inequality() {
        fn prop(a: Key<NodeId>, b: Key<NodeId>, c: Key<NodeId>) -> TestResult {
            let ab = a.distance(&b);
            let bc = b.distance(&c);
            let (ab_plus_bc, overflow) = ab.0.overflowing_add(bc.0);
            if overflow {
                TestResult::discard()
            } else {
                TestResult::from_bool(a.distance(&c) <= Distance(ab_plus_bc))
            }
        }
        quickcheck(prop as fn(_, _, _) -> _)
    }

    #[test]
    fn unidirectionality() {
        fn prop(a: Key<NodeId>, b: Key<NodeId>) -> bool {
            let d = a.distance(&b);
            (0..100).all(|_| {
                let c = Key::from(NodeId::random());
                a.distance(&c) != d || b == c
            })
        }
        quickcheck(prop as fn(_, _) -> _)
    }
}
