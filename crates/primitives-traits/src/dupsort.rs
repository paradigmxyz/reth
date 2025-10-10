//! DupSort value trait for database tables.

/// Trait for values that can be used in DupSort tables.
/// These values must be able to provide their subkey.
pub trait DupSortValue<SubKey> {
    /// Returns the subkey for this value.
    fn subkey(&self) -> &SubKey;
}
