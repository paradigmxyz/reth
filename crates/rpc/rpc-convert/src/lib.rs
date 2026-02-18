//! Reth compatibility and utils for RPC types
//!
//! This crate various helper functions to convert between reth primitive types and rpc types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod rpc;
pub mod rpc_response;
pub mod transaction;

pub use rpc::*;
pub use rpc_response::{EthRpcConverter, RpcResponseConverter, RpcResponseConverterError};
pub use transaction::{
    RpcConvert, RpcConverter, TransactionConversionError, TryIntoSimTx, TxInfoMapper,
};

pub use alloy_evm::rpc::{CallFees, CallFeesError, EthTxEnvError, TryIntoTxEnv};
