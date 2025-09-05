use alloc::vec::Vec;
use alloy_consensus::{
    transaction::TxEip4844Sidecar, EthereumTxEnvelope, Header, TxEip1559, TxEip2930, TxEip4844,
    TxEip4844Variant, TxEip4844WithSidecar, TxEip7702, TxLegacy, TxType,
};
use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::{Signature, TxHash, B256};
use revm_primitives::Log;

/// Trait for calculating a heuristic for the in-memory size of a struct.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait InMemorySize {
    /// Returns a heuristic for the in-memory size of a struct.
    fn size(&self) -> usize;
}

impl<T: InMemorySize> InMemorySize for alloy_consensus::Signed<T> {
    fn size(&self) -> usize {
        T::size(self.tx()) + self.signature().size() + core::mem::size_of::<B256>()
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

impl_in_mem_size_size_of!(Signature, TxHash, TxType);

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

impl<T: TxEip4844Sidecar> InMemorySize for TxEip4844Variant<T> {
    #[inline]
    fn size(&self) -> usize {
        Self::size(self)
    }
}

impl<T: TxEip4844Sidecar> InMemorySize for TxEip4844WithSidecar<T> {
    #[inline]
    fn size(&self) -> usize {
        Self::size(self)
    }
}

#[cfg(feature = "op")]
impl_in_mem_size_size_of!(op_alloy_consensus::OpTxType);

impl InMemorySize for alloy_consensus::Receipt {
    fn size(&self) -> usize {
        let Self { status, cumulative_gas_used, logs } = self;
        core::mem::size_of_val(status) +
            core::mem::size_of_val(cumulative_gas_used) +
            logs.capacity() * core::mem::size_of::<Log>()
    }
}

impl<T: InMemorySize> InMemorySize for EthereumTxEnvelope<T> {
    fn size(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.size(),
            Self::Eip2930(tx) => tx.size(),
            Self::Eip1559(tx) => tx.size(),
            Self::Eip4844(tx) => tx.size(),
            Self::Eip7702(tx) => tx.size(),
        }
    }
}

impl<T: InMemorySize, H: InMemorySize> InMemorySize for alloy_consensus::BlockBody<T, H> {
    /// Calculates a heuristic for the in-memory size of the block body
    #[inline]
    fn size(&self) -> usize {
        self.transactions.iter().map(T::size).sum::<usize>() +
            self.transactions.capacity() * core::mem::size_of::<T>() +
            self.ommers.iter().map(H::size).sum::<usize>() +
            self.ommers.capacity() * core::mem::size_of::<Header>() +
            self.withdrawals
                .as_ref()
                .map_or(core::mem::size_of::<Option<Withdrawals>>(), Withdrawals::total_size)
    }
}

impl<T: InMemorySize, H: InMemorySize> InMemorySize for alloy_consensus::Block<T, H> {
    #[inline]
    fn size(&self) -> usize {
        self.header.size() + self.body.size()
    }
}

impl<T: InMemorySize> InMemorySize for Vec<T> {
    fn size(&self) -> usize {
        // Note: This does not track additional capacity
        self.iter().map(T::size).sum::<usize>()
    }
}

impl InMemorySize for u64 {
    fn size(&self) -> usize {
        core::mem::size_of::<Self>()
    }
}

/// Implementation for optimism types
#[cfg(feature = "op")]
mod op {
    use super::*;

    impl InMemorySize for op_alloy_consensus::OpDepositReceipt {
        fn size(&self) -> usize {
            let Self { inner, deposit_nonce, deposit_receipt_version } = self;
            inner.size() +
                core::mem::size_of_val(deposit_nonce) +
                core::mem::size_of_val(deposit_receipt_version)
        }
    }

    impl InMemorySize for op_alloy_consensus::OpTypedTransaction {
        fn size(&self) -> usize {
            match self {
                Self::Legacy(tx) => tx.size(),
                Self::Eip2930(tx) => tx.size(),
                Self::Eip1559(tx) => tx.size(),
                Self::Eip7702(tx) => tx.size(),
                Self::Deposit(tx) => tx.size(),
            }
        }
    }

    impl InMemorySize for op_alloy_consensus::OpPooledTransaction {
        fn size(&self) -> usize {
            match self {
                Self::Legacy(tx) => tx.size(),
                Self::Eip2930(tx) => tx.size(),
                Self::Eip1559(tx) => tx.size(),
                Self::Eip7702(tx) => tx.size(),
            }
        }
    }

    impl InMemorySize for op_alloy_consensus::OpTxEnvelope {
        fn size(&self) -> usize {
            match self {
                Self::Legacy(tx) => tx.size(),
                Self::Eip2930(tx) => tx.size(),
                Self::Eip1559(tx) => tx.size(),
                Self::Eip7702(tx) => tx.size(),
                Self::Deposit(tx) => tx.size(),
            }
        }
    }
}
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
