//! Generic value encoder types for proof calculation with lazy evaluation.

use alloy_primitives::U256;
use alloy_rlp::Encodable;
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::RlpNode;

/// A trait for deferred RLP encoding of proof values.
///
/// This trait is implemented by types that can encode themselves into an RLP buffer
/// on demand, allowing for lazy computation of storage roots and other deferred work.
pub trait RlpNodeFut {
    /// Encodes the value as RLP into the provided buffer and returns the RlpNode.
    ///
    /// # Arguments
    ///
    /// * `buf` - A mutable buffer to encode the RLP data into
    ///
    /// # Returns
    ///
    /// The RlpNode representation of the encoded data
    fn get(self, buf: &mut Vec<u8>) -> Result<RlpNode, StateProofError>;
}

/// A trait for encoding values into RLP nodes for proof calculation.
///
/// This trait allows for lazy evaluation of RLP encoding, enabling deferred
/// computation of storage roots and other expensive operations.
///
/// The encoder takes a reference to itself and a value, returning a future-like
/// type that will perform the encoding when needed.
pub trait ValueEncoder {
    /// The type of value being encoded (e.g., U256 for storage, Account for accounts).
    type Value;

    /// The type that will compute and encode the RLP node when needed.
    type RlpNodeFut: RlpNodeFut;

    /// Returns a future-like value that will compute the RLP node when called.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to encode
    ///
    /// The returned future should be called as late as possible in the algorithm to maximize
    /// the time available for parallel computation (e.g., storage root calculation).
    fn rlp_fut(&self, value: Self::Value) -> Self::RlpNodeFut;
}

/// An encoder for storage slot values.
///
/// This encoder simply RLP-encodes U256 storage values directly.
#[derive(Debug, Clone, Copy, Default)]
pub struct StorageValueEncoder;

/// The RLP encoding future for a storage slot value.
#[derive(Debug, Clone, Copy)]
pub struct StorageRlpNodeFut(U256);

impl RlpNodeFut for StorageRlpNodeFut {
    fn get(self, buf: &mut Vec<u8>) -> Result<RlpNode, StateProofError> {
        buf.clear();
        self.0.encode(buf);
        Ok(RlpNode::from_rlp(buf))
    }
}

impl ValueEncoder for StorageValueEncoder {
    type Value = U256;
    type RlpNodeFut = StorageRlpNodeFut;

    fn rlp_fut(&self, value: Self::Value) -> Self::RlpNodeFut {
        StorageRlpNodeFut(value)
    }
}

// /// An account value that includes account data and optional storage proof calculation.
// ///
// /// For account proofs, the default implementation will synchronously call a storage
// /// proof with empty targets to compute the storage root. This design allows for:
// /// - Injecting parallelization of storage root calculation in later iterations
// /// - Caching of storage roots in later iterations
// #[derive(Debug)]
// pub struct AccountValue<F>
// where
//     F: FnOnce() -> Result<B256, StateProofError> + Send,
// {
//     /// The account data (without storage root).
//     pub account: Account,
//     /// A closure that computes the storage root when called.
//     /// For the default implementation, this calls a storage proof with empty targets.
//     pub storage_root_fn: F,
// }
//
// impl<F> AccountValue<F>
// where
//     F: FnOnce() -> Result<B256, StateProofError> + Send + 'static,
// {
//     /// Create a new account value with the given account and storage root function.
//     pub const fn new(account: Account, storage_root_fn: F) -> Self {
//         Self { account, storage_root_fn }
//     }
// }
//
// /// The RLP encoding future for an account value.
// #[derive(Debug)]
// pub struct AccountRlpNodeFut<F>
// where
//     F: FnOnce() -> Result<B256, StateProofError> + Send,
// {
//     account: Account,
//     storage_root_fn: F,
// }
//
// impl<F> RlpNodeFut for AccountRlpNodeFut<F>
// where
//     F: FnOnce() -> Result<B256, StateProofError> + Send,
// {
//     fn get(self, buf: &mut Vec<u8>) -> Result<RlpNode, StateProofError> {
//         // Call the storage root function to get the storage root
//         let storage_root = (self.storage_root_fn)()?;
//
//         // Combine account with storage root to create TrieAccount
//         let trie_account = self.account.into_trie_account(storage_root);
//
//         // Encode the trie account
//         buf.clear();
//         trie_account.encode(buf as &mut dyn BufMut);
//
//         // Convert to RlpNode
//         Ok(RlpNode::from_rlp(buf))
//     }
// }
//
// impl<F> ValueEncoder for AccountValue<F>
// where
//     F: FnOnce() -> Result<B256, StateProofError> + Send + 'static,
// {
//     type Value = Account;
//     type RlpNodeFut = AccountRlpNodeFut<F>;
//
//     fn rlp_fut(&self, value: Self::Value) -> Self::RlpNodeFut {
//         AccountRlpNodeFut { account: value, storage_root_fn: self.storage_root_fn }
//     }
// }
