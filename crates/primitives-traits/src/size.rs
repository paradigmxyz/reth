use alloy_consensus::{Header, TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy};
use alloy_primitives::{PrimitiveSignature as Signature, TxHash};

/// Trait for calculating a heuristic for the in-memory size of a struct.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait InMemorySize {
    /// Returns a heuristic for the in-memory size of a struct.
    fn size(&self) -> usize;
}

impl<T: InMemorySize> InMemorySize for alloy_consensus::Signed<T> {
    fn size(&self) -> usize {
        T::size(self.tx()) + self.signature().size() + self.hash().size()
    }
}

/// Implement `InMemorySize` for a type with `size_of`
macro_rules! impl_in_mem_size_size_of {
    ($($ty:ty),*) => {
        $(
            impl InMemorySize for $ty {
                #[inline]
                fn size(&self) -> usize {
                    core::mem::size_of::<Self>()
                }
            }
        )*
    };
}

impl_in_mem_size_size_of!(Signature, TxHash);

/// Implement `InMemorySize` for a type with a native `size` method.
macro_rules! impl_in_mem_size {
    ($($ty:ty),*) => {
        $(
            impl InMemorySize for $ty {
                #[inline]
                fn size(&self) -> usize {
                   Self::size(self)
                }
            }
        )*
    };
}

impl_in_mem_size!(Header, TxLegacy, TxEip2930, TxEip1559, TxEip7702, TxEip4844);

#[cfg(test)]
mod tests {
    use super::*;

    // ensures we don't have any recursion in the `InMemorySize` impls
    #[test]
    fn no_in_memory_no_recursion() {
        fn assert_no_recursion<T: InMemorySize + Default>() {
            let _ = T::default().size();
        }
        assert_no_recursion::<Header>();
        assert_no_recursion::<TxLegacy>();
        assert_no_recursion::<TxEip2930>();
        assert_no_recursion::<TxEip1559>();
        assert_no_recursion::<TxEip7702>();
        assert_no_recursion::<TxEip4844>();
    }
}
