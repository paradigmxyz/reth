//! Helpers for recovering signers from a set of transactions

#[cfg(feature = "rayon")]
pub use rayon::*;

#[cfg(not(feature = "rayon"))]
pub use iter::*;

#[cfg(feature = "rayon")]
mod rayon {
    use crate::SignedTransaction;
    use alloc::vec::Vec;
    use alloy_primitives::Address;
    use rayon::prelude::{IntoParallelIterator, ParallelIterator};

    /// Recovers a list of signers from a transaction list iterator.
    ///
    /// Returns `None`, if some transaction's signature is invalid
    pub fn recover_signers<'a, I, T>(txes: I) -> Option<Vec<Address>>
    where
        T: SignedTransaction,
        I: IntoParallelIterator<Item = &'a T> + IntoIterator<Item = &'a T> + Send,
    {
        txes.into_par_iter().map(|tx| tx.recover_signer()).collect()
    }

    /// Recovers a list of signers from a transaction list iterator _without ensuring that the
    /// signature has a low `s` value_.
    ///
    /// Returns `None`, if some transaction's signature is invalid.
    pub fn recover_signers_unchecked<'a, I, T>(txes: I) -> Option<Vec<Address>>
    where
        T: SignedTransaction,
        I: IntoParallelIterator<Item = &'a T> + IntoIterator<Item = &'a T> + Send,
    {
        txes.into_par_iter().map(|tx| tx.recover_signer_unchecked()).collect()
    }
}

#[cfg(not(feature = "rayon"))]
mod iter {
    use crate::SignedTransaction;
    use alloc::vec::Vec;
    use alloy_primitives::Address;

    /// Recovers a list of signers from a transaction list iterator.
    ///
    /// Returns `None`, if some transaction's signature is invalid
    pub fn recover_signers<'a, I, T>(txes: I) -> Option<Vec<Address>>
    where
        T: SignedTransaction,
        I: IntoIterator<Item = &'a T> + IntoIterator<Item = &'a T>,
    {
        txes.into_iter().map(|tx| tx.recover_signer()).collect()
    }

    /// Recovers a list of signers from a transaction list iterator _without ensuring that the
    /// signature has a low `s` value_.
    ///
    /// Returns `None`, if some transaction's signature is invalid.
    pub fn recover_signers_unchecked<'a, I, T>(txes: I) -> Option<Vec<Address>>
    where
        T: SignedTransaction,
        I: IntoIterator<Item = &'a T> + IntoIterator<Item = &'a T>,
    {
        txes.into_iter().map(|tx| tx.recover_signer_unchecked()).collect()
    }
}
