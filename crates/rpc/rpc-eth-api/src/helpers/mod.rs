//! Behaviour needed to serve `eth_` RPC requests, divided into general database reads and
//! specific database access.
//!
//! Traits with `Load` prefix, read atomic data from database, e.g. a block or transaction. Any
//! database read done in more than one default `Eth` trait implementation, is defined in a `Load`
//! trait.
//!
//! Traits with `Eth` prefix, compose specific data needed to serve RPC requests in the `eth`
//! namespace. They use `Load` traits as building blocks. [`EthTransactions`] also writes data
//! (submits transactions). Based on the `eth_` request method semantics, request methods are
//! divided into: [`EthTransactions`], [`EthBlocks`], [`EthFees`], [`EthState`] and [`EthCall`].
//! Default implementation of the `Eth` traits, is done w.r.t. L1.
//!
//! [`EthApiServer`](crate::EthApiServer), is implemented for any type that implements
//! all the `Eth` traits, e.g. `reth_rpc::EthApi`.

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

pub use block::{EthBlocks, LoadBlock};
pub use blocking_task::SpawnBlocking;
pub use call::{Call, EthCall};
pub use fee::{EthFees, LoadFee};
pub use pending_block::LoadPendingBlock;
pub use receipt::LoadReceipt;
pub use signer::{AddDevSigners, EthSigner};
pub use spec::EthApiSpec;
pub use state::{EthState, LoadState};
pub use trace::Trace;
pub use transaction::{EthTransactions, LoadTransaction, UpdateRawTxForwarder};

use crate::EthApiTypes;

/// Extension trait that bundles traits needed for tracing transactions.
pub trait TraceExt<T: EthApiTypes>:
    LoadTransaction<T> + LoadBlock<T> + LoadPendingBlock<T> + SpawnBlocking<T> + Trace<T> + Call<T>
{
}

impl<T: EthApiTypes> TraceExt<T> for T where
    T: LoadTransaction<T> + LoadBlock<T> + LoadPendingBlock<T> + Trace<T> + Call<T>
{
}

/// Helper trait to unify all `eth` rpc server building block traits, for simplicity.
///
/// This trait is automatically implemented for any type that implements all the `Eth` traits.
pub trait FullEthApi<T: EthApiTypes>:
    EthApiSpec
    + EthTransactions<T>
    + EthBlocks<T>
    + EthState<T>
    + EthCall<T>
    + EthFees<T>
    + Trace<T>
    + LoadReceipt<T>
{
}

impl<T, EthApi> FullEthApi<T> for EthApi
where
    T: EthApiTypes,
    EthApi: EthApiSpec
        + EthTransactions<T>
        + EthBlocks<T>
        + EthState<T>
        + EthCall<T>
        + EthFees<T>
        + Trace<T>
        + LoadReceipt<T>,
{
}
