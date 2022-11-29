//! Integrates different codecs into table::Encode and table::Decode

mod compact;
pub use compact::CompactU256;

pub mod fuzz;
mod postcard;
#[cfg(not(feature = "bench-postcard"))]
mod scale;
