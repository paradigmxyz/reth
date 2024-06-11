//! Database access.

pub mod block;
pub mod blocking_task;
pub mod call;
pub mod fee;
pub mod pending_block;
pub mod receipt;
pub mod state;
pub mod trace;
pub mod transaction;

pub use block::{EthBlocks, LoadBlock};
pub use blocking_task::SpawnBlocking;
pub use call::{Call, EthCall, StateCacheDB};
pub use fee::{EthFees, LoadFee};
pub use pending_block::LoadPendingBlock;
pub use receipt::BuildReceipt;
pub use state::{EthState, LoadState};
pub use trace::Trace;
pub use transaction::{EthTransactions, LoadTransaction, RawTransactionForwarder};

/// Extension trait that bundles traits needed for loading execution environment.
pub trait LoadStateExt: LoadState + LoadPendingBlock + SpawnBlocking {}

impl<T> LoadStateExt for T where T: LoadState + LoadPendingBlock + SpawnBlocking {}

/// Extension trait that bundles traits needed for loading a block at any point in finalization.
pub trait LoadBlockExt: LoadBlock + LoadPendingBlock + SpawnBlocking {}

impl<T> LoadBlockExt for T where T: LoadBlock + LoadPendingBlock + SpawnBlocking {}

/// Extension trait that bundles traits needed for tracing transactions.
pub trait TraceExt: LoadStateExt + Trace + Call {}

impl<T> TraceExt for T where T: LoadStateExt + Trace + Call {}
