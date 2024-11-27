//! Block body abstraction.

use alloc::vec::Vec;
use core::fmt;
#[cfg(feature = "std")]
use std::sync::LazyLock;

use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::Address;
#[cfg(feature = "std")]
use once_cell as _;
#[cfg(not(feature = "std"))]
use once_cell::sync::Lazy as LazyLock;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::{FullSignedTx, InMemorySize, MaybeArbitrary, MaybeSerde, SignedTransaction};

/// Expected number of transactions where we can expect a speed-up by recovering the senders in
/// parallel.
pub static PARALLEL_SENDER_RECOVERY_THRESHOLD: LazyLock<usize> =
    LazyLock::new(|| match rayon::current_num_threads() {
        0..=1 => usize::MAX,
        2..=8 => 10,
        _ => 5,
    });

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullBlockBody: BlockBody<Transaction: FullSignedTx> {}

impl<T> FullBlockBody for T where T: BlockBody<Transaction: FullSignedTx> {}

/// Abstraction for block's body.
pub trait BlockBody:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + PartialEq
    + Eq
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + InMemorySize
    + MaybeSerde
    + MaybeArbitrary
{
    /// Ordered list of signed transactions as committed in block.
    type Transaction: SignedTransaction;

    /// Ommer header type.
    type OmmerHeader;

    /// Returns reference to transactions in block.
    fn transactions(&self) -> &[Self::Transaction];

    /// Consume the block body and return a [`Vec`] of transactions.
    fn into_transactions(self) -> Vec<Self::Transaction>;

    /// Returns block withdrawals if any.
    fn withdrawals(&self) -> Option<&Withdrawals>;

    /// Returns block ommers if any.
    fn ommers(&self) -> Option<&[Self::OmmerHeader]>;

    /// Recover signer addresses for all transactions in the block body.
    fn recover_signers(&self) -> Option<Vec<Address>> {
        if self.transactions().len() < *PARALLEL_SENDER_RECOVERY_THRESHOLD {
            self.transactions().iter().map(|tx| tx.recover_signer()).collect()
        } else {
            self.transactions().par_iter().map(|tx| tx.recover_signer()).collect()
        }
    }
}
