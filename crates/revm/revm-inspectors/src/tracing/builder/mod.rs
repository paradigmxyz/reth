//! Builder types for building traces

/// Geth style trace builders for `debug_` namespace
pub mod geth;

/// Parity style trace builders for `trace_` namespace
pub mod parity;

/// Walker trait and types used for walking various callgraphs
mod walker;
