//! Network-level state cache for Partial Statelessness PoC.
//!
//! This crate implements a **protocol-level** cache that represents the state subset
//! all network validators are assumed to hold. It is completely separate from reth's
//! internal `ExecutionCache` which optimizes local DB I/O.
//!
//! The cache supports separate eviction policies for accounts vs storage/codes,
//! and tracks which state keys would require a witness (Merkle proof) when a new
//! block arrives.

pub mod accessed_state;
pub mod network_cache;
pub mod persistence;
pub mod policy;

pub use accessed_state::BlockAccessedState;
pub use network_cache::{CachedEntry, NetworkStateCache};
pub use policy::{CachePolicy, LastNBlocksPolicy};
