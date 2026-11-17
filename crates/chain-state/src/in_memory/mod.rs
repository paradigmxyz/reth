//! Types for tracking the canonical chain state in memory.

mod block_state;
pub use block_state::BlockState;

mod blocks;

mod canonical;
pub use canonical::CanonicalInMemoryState;

mod executed_block;
pub use executed_block::ExecutedBlock;

mod new_canonical_chain;
pub use new_canonical_chain::NewCanonicalChain;
