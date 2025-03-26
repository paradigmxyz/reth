/// Transaction Type
///
/// Currently being used as 2-bit type when encoding it to `reth_codecs::Compact` on
/// [`crate::TransactionSigned`]. Adding more transaction types will break the codec and
/// database format.
///
/// Other required changes when adding a new type can be seen on [PR#3953](https://github.com/paradigmxyz/reth/pull/3953/files).
pub use alloy_consensus::TxType;
