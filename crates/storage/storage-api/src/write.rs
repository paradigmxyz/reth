use reth_storage_errors::db::DatabaseError;

/// A type that can be written to storage.
pub trait Writeable {
    /// The type with access to the storage backend.
    type StorageAccess<'a>;

    /// Write the data to the storage backend.
    fn flush(self, storage: Self::StorageAccess<'_>) -> Result<(), DatabaseError>;

}

/// A type that can create types that have access to the storage backend.
pub trait StorageAccessProvider<S> {

    /// Create a storage input type from the storage backend.
    fn storage(&self) -> S;
}


/// A type that can write data to storage.
pub trait StorageWriter: Send + Sync {

    /// Flush the data to the storage backend.
    fn flush<T>(&self, data: T) -> Result<(), DatabaseError>
        where
            Self: for<'a> StorageAccessProvider<T::StorageAccess<'a>>,
            T: Writeable {
        let storage = self.storage();
        data.flush(storage)
    }

}