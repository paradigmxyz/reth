//! Integrates different codecs into `table::Encode` and `table::Decode`.

mod compact;
pub use compact::CompactU256;

pub mod fuzz;

mod scale;
