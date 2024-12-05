//! Standalone crate for Optimism-specific Reth primitive types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
// The `optimism` feature must be enabled to use this crate.
#![cfg(feature = "optimism")]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod bedrock;
pub mod transaction;

pub use transaction::{signed::OpTransactionSigned, tx_type::OpTxType, OpTransaction};

/// Optimism primitive types.
pub type OpPrimitives = reth_primitives::EthPrimitives;

// TODO: once we are ready for separating primitive types, introduce a separate `NodePrimitives`
// implementation used exclusively by legacy engine.
//
// #[derive(Debug, Default, Clone, PartialEq, Eq)]
// pub struct OpPrimitives;
//
// impl NodePrimitives for OpPrimitives {
//     type Block = Block;
//     type BlockHeader = Header;
//     type BlockBody = BlockBody;
//     type SignedTx = TransactionSigned;
//     type TxType = OpTxType;
//     type Receipt = Receipt;
// }
