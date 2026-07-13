//! BAL-driven parallel block execution.

mod ordered_outputs;
mod worker;

pub mod error;
pub mod execute;

pub use error::BalExecutionError;
pub use execute::execute_block;
