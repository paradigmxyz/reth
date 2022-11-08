//! Integrates different codecs into table::Encode and table::Decode

mod compact;
pub mod fuzz;
mod postcard;
#[cfg(not(feature = "bench-postcard"))]
mod scale;
