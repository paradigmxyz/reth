//! Behaviour needed to serve `eth_` RPC requests, divided into general database reads and
//! specific database access.
//!
//! Traits with `Load` prefix, read atomic data from database, e.g. a block or transaction. Any
//! database read done in more than one default `Eth` trait implementation, is defined in a `Load`
//! trait.
//!
//! Traits with `Eth` prefix, compose specific data needed to serve RPC requests in the `eth`
//! namespace. They use `Load` traits as building blocks.
//! [`EthTransactions` also writes data (submits transactions).
//! Based on the `eth_` request method semantics, request methods are divided into:
//! [`EthTransactions`], [`EthBlocks`], [`EthFees`], [`EthState`] and [`EthCall`]. Default
//! implementation of the `Eth` traits, is done w.r.t. L1.
//!
//! [`EthApiServer`](crate::EthApiServer), is implemented for any type that implements
//! all the `Eth` traits, e.g. [`EthApi`](crate::EthApi).

pub mod block;
pub mod blocking_task;
pub mod call;
pub mod fee;
pub mod pending_block;
pub mod receipt;
pub mod signer;
pub mod spec;
pub mod state;
pub mod trace;
pub mod transaction;

use blocking_task::SpawnBlocking;
use call::Call;
use pending_block::LoadPendingBlock;
use trace::Trace;

/// Extension trait that bundles traits needed for tracing transactions.
pub trait TraceExt: LoadPendingBlock + SpawnBlocking + Trace + Call {}

impl<T> TraceExt for T where T: LoadPendingBlock + Trace + Call {}

use block::EthBlocks;
use call::EthCall;
use fee::EthFees;
use receipt::LoadReceipt;
use spec::EthApiSpec;
use state::EthState;
use transaction::EthTransactions;

/// Helper trait to unify all `eth` rpc server building block traits, for simplicity.
pub trait FullEthServer:
    EthApiSpec + EthTransactions + EthBlocks + EthState + EthCall + EthFees + Trace + LoadReceipt
{
}

impl<T> FullEthServer for T where
    T: EthApiSpec
        + EthTransactions
        + EthBlocks
        + EthState
        + EthCall
        + EthFees
        + Trace
        + LoadReceipt
{
}
