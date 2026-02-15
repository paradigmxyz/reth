//! Snap sync protocol implementation for reth.
//!
//! Downloads state from peers via the [snap protocol](https://github.com/ethereum/devp2p/blob/master/caps/snap.md)
//! instead of executing all historical blocks. The sync proceeds in phases:
//!
//! 1. **Account download**: Fetch all account leaves via `GetAccountRange`
//! 2. **Storage download**: Fetch storage slots for accounts with non-empty storage roots
//! 3. **Bytecode download**: Fetch contract bytecodes by code hash
//! 4. **State root verification**: Build hashed state + merkle trie, verify against pivot block
//!
//! After snap sync completes, normal execution resumes from the pivot block onward.

pub mod downloader;
pub mod error;
pub mod metrics;
pub mod progress;

pub use downloader::SnapSyncDownloader;
pub use error::SnapSyncError;
pub use progress::{SnapPhase, SnapProgress};
