//! Port of revm's blockchain state that can be safely read and shared across threads.

mod cache_account;
pub use cache_account::SharedCacheAccount;

mod cache;
pub use cache::SharedCacheState;

mod state;
pub use state::{SharedState, SharedStateLock};
