//! crates/transaction-pool/src/traits/blob.rs

use super::TransactionPool;

/// Marker trait for pools that expose **blob‑transaction** helpers.
///
/// *Empty in PR 1 – methods are added in the follow‑up patch.*
#[auto_impl::auto_impl(&, Arc)]
pub trait BlobPoolExt: TransactionPool {}