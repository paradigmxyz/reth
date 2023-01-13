use std::collections::BinaryHeap;

use derive_more::{Deref, DerefMut};

macro_rules! bounded_collection {
    ($name:ident, $inner:ident $(: $constr:ident $(+ $other_constr:ident)* )?) => {
        /// Implementation of a bounded binary heap decorated with capacity.
        #[derive(Deref, DerefMut, Debug)]
        pub struct $name<T> {
            #[deref]
            #[deref_mut]
            inner: $inner<T>,
            capacity: usize,
        }

        impl<T$(: $constr $(+ $other_constr)*)?> $name<T> {
            /// Create new instance of bounded collection.
            pub fn new(capacity: usize) -> Self {
                Self { inner: $inner::with_capacity(capacity), capacity }
            }

            /// Returns `true` if length exceeds initial capacity.
            pub fn at_capacity(&self) -> bool {
                self.len() >= self.capacity
            }

            /// Attempts to push item to the inner collection.
            /// Returns an error if collection is at its capacity.
            pub fn push_within_capacity(&mut self, item: T) -> Result<(), T> {
                if self.at_capacity() {
                    Err(item)
                } else {
                    self.inner.push(item);
                    Ok(())
                }
            }
        }
    };
}

// NOTE: Can be removed `push_within_capacity` is stabilized.
// https://doc.rust-lang.org/std/vec/struct.Vec.html#method.push_within_capacity
bounded_collection!(BoundedVec, Vec);

bounded_collection!(BoundedBinaryHeap, BinaryHeap: Ord);
