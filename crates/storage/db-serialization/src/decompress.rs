use core::fmt::Debug;
use crate::DecodeError;

/// Trait that will transform the data to be read from the DB.
pub trait Decompress: Send + Sync + Sized + Debug {
    /// Decompresses data coming from the database.
    fn decompress<B: AsRef<[u8]>>(value: B) -> Result<Self, DecodeError>;

    /// Decompresses owned data coming from the database.
    fn decompress_owned(value: Vec<u8>) -> Result<Self, DecodeError> {
        Self::decompress(value)
    }
}