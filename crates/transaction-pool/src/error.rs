//! Transaction pool errors

/// Transaction pool result type.
pub type Result<T> = std::result::Result<T, Error>;

/// All errors the Transaction pool can throw.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Thrown if a replacement transaction's gas price is below the already imported transaction
    #[error("Tx: insufficient gas price to replace existing transaction")]
    // #[error("Tx: [{0:?}] insufficient gas price to replace existing transaction")]
    // ReplacementUnderpriced(Box<PoolTransaction>),
    ReplacementUnderpriced,
    // TODO make error generic over `Transaction`
}
