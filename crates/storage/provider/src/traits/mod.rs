//! Collection of common provider traits.

mod account;
pub use account::AccountProvider;

mod block;
pub use block::BlockProvider;

mod block_hash;
pub use block_hash::BlockHashProvider;

mod header;
pub use header::HeaderProvider;

mod state;
pub use state::{StateProvider, StateProviderFactory};
