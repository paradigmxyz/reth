#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! A [Consensus](reth_interfaces::consensus::Consensus) implementation for local testing purposes
//! that automatically seals blocks.

use reth_interfaces::consensus::ForkchoiceState;

mod mode;

/// A consensus implementation that follows a strategy for announcing blocks via [ForkchoiceState]
#[derive(Debug)]
pub struct AutoSealConsensus {}
