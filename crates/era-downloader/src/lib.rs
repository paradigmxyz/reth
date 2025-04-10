//! An asynchronous stream interface for downloading ERA1 files.
mod client;
mod stream;

pub use client::EraClient;
pub use stream::EraStream;
