//! Transaction abstraction

pub mod signed;

use alloc::fmt;

use reth_codecs::Compact;
use serde::{Deserialize, Serialize};

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullTransaction: Transaction + Compact {}

impl<T> FullTransaction for T where T: Transaction + Compact {}

/// Abstraction of a transaction.
pub trait Transaction:
    alloy_consensus::Transaction
    + Clone
    + fmt::Debug
    + PartialEq
    + Eq
    + Default
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + Serialize
    + for<'de> Deserialize<'de>
{
}
