use alloc::{vec, vec::Vec};
use alloy_primitives::B256;
use derive_more::{DerefMut, From, IntoIterator};
use serde::{Deserialize, Serialize};

/// Retrieves gas spent by transactions as a vector of tuples (transaction index, gas used).
pub use reth_primitives_traits::receipt::gas_spent_by_transactions;

/// Receipt containing result of transaction execution.
pub use reth_ethereum_primitives::Receipt;

/// A collection of receipts organized as a two-dimensional vector.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    From,
    derive_more::Deref,
    DerefMut,
    IntoIterator,
)]
pub struct Receipts<T = Receipt> {
    /// A two-dimensional vector of optional `Receipt` instances.
    pub receipt_vec: Vec<Vec<T>>,
}

impl<T> Receipts<T> {
    /// Returns the length of the `Receipts` vector.
    pub fn len(&self) -> usize {
        self.receipt_vec.len()
    }

    /// Returns true if the `Receipts` vector is empty.
    pub fn is_empty(&self) -> bool {
        self.receipt_vec.is_empty()
    }

    /// Push a new vector of receipts into the `Receipts` collection.
    pub fn push(&mut self, receipts: Vec<T>) {
        self.receipt_vec.push(receipts);
    }

    /// Retrieves all recorded receipts from index and calculates the root using the given closure.
    pub fn root_slow(&self, index: usize, f: impl FnOnce(&[&T]) -> B256) -> B256 {
        let receipts = self.receipt_vec[index].iter().collect::<Vec<_>>();
        f(receipts.as_slice())
    }
}

impl<T> From<Vec<T>> for Receipts<T> {
    fn from(block_receipts: Vec<T>) -> Self {
        Self { receipt_vec: vec![block_receipts.into_iter().collect()] }
    }
}

impl<T> FromIterator<Vec<T>> for Receipts<T> {
    fn from_iter<I: IntoIterator<Item = Vec<T>>>(iter: I) -> Self {
        iter.into_iter().collect::<Vec<_>>().into()
    }
}

impl<T> Default for Receipts<T> {
    fn default() -> Self {
        Self { receipt_vec: Vec::new() }
    }
}
