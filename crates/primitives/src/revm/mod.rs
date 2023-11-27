/// The `compat` module contains a set of utility functions that bridge the gap between Revm and
/// Reth Ethereum implementations.
///
/// These functions enable the conversion of data structures between the two implementations, such
/// as converting `Log` structures, `AccountInfo`, and `Account` objects.
///
/// Additionally, it provides a function to calculate intrinsic gas usage for transactions beyond
/// the Merge hardfork, offering compatibility for both Shanghai and Merge Ethereum specifications.
///
/// These utilities facilitate interoperability and data exchange between Revm and Reth
/// implementations.
pub mod compat;
