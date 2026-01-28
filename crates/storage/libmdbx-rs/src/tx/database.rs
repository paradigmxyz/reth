use crate::flags::DatabaseFlags;

/// A handle to an individual database in an environment.
///
/// A database handle denotes the name and parameters of a database in an
/// environment.
///
/// `Database` is a simple data container holding the database handle index
/// (dbi) and its flags. It does not own any resources and can be freely
/// copied.
///
/// # Lifetime
///
/// The database handle is only valid within the lifetime of the environment
/// that created it. Users must ensure that `Database` instances are not used
/// after the environment has been closed.
#[derive(Debug, Clone, Copy)]
pub struct Database {
    dbi: ffi::MDBX_dbi,
    flags: DatabaseFlags,
}

impl Database {
    /// Creates a new Database from a dbi and flags.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the dbi is valid and was obtained from
    /// the same environment that will be used to access this database.
    pub const fn new(dbi: ffi::MDBX_dbi, flags: DatabaseFlags) -> Self {
        Self { dbi, flags }
    }

    /// Opens the freelist database with DBI `0`.
    pub const fn freelist_db() -> Self {
        Self { dbi: 0, flags: DatabaseFlags::empty() }
    }

    /// Returns the underlying MDBX database handle.
    ///
    /// The caller **must** ensure that the handle is not used after the
    /// lifetime of the environment, or after the database has been closed.
    pub const fn dbi(&self) -> ffi::MDBX_dbi {
        self.dbi
    }

    /// Returns the database flags.
    pub const fn flags(&self) -> DatabaseFlags {
        self.flags
    }
}
