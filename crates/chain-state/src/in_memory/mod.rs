//! Types for tracking the executed blocks that are not on disk yet, and the canonical chain among
//! them.

mod block_state;
pub use block_state::BlockState;

mod blocks;

mod canonical;
pub use canonical::{CanonicalInMemoryState, CanonicalInMemoryStateWriter};

mod executed_block;
pub use executed_block::ExecutedBlock;

mod new_canonical_chain;
pub use new_canonical_chain::NewCanonicalChain;

mod state;
pub use state::{
    InMemoryCanonicalBlocksRemoval, InMemoryState, InMemoryStateBlockCounts, InMemoryStateWriter,
};
