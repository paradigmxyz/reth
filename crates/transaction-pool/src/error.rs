//! Transaction pool errors

/// Transaction pool result type.
pub type Result<T> = std::result::Result<T, Error>;

/// All errors the Transaction pool can throw.
#[derive(Debug, thiserror::Error)]
pub enum Error {}
