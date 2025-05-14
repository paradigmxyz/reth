//! Scroll consensus implementation.

extern crate alloc;

mod error;
pub use error::ScrollConsensusError;

mod validation;

pub use validation::ScrollBeaconConsensus;
