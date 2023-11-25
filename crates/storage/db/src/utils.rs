//! Utils crate for `db`.
use std::{fs, path::Path};

// Constants for page size calculations
// source: https://gitflic.ru/project/erthink/libmdbx/blob?file=mdbx.h#line-num-821
const LIBMDBX_MAX_PAGE_SIZE: usize = 0x10000; // Maximum page size defined by libmdbx
const MIN_PAGE_SIZE: usize = 4096; // Minimum page size threshold

/// Returns the default page size that can be used in this OS.
pub(crate) fn default_page_size() -> usize {
    let os_page_size = page_size::get();
    os_page_size.clamp(MIN_PAGE_SIZE, LIBMDBX_MAX_PAGE_SIZE)
}

/// Check if a db is empty. It does not provide any information on the
/// validity of the data in it. We consider a database as non empty when it's a non empty directory.
pub fn is_database_empty<P: AsRef<Path>>(path: P) -> bool {
    match fs::read_dir(path.as_ref()) {
        Ok(mut dir) => dir.next().is_none(),
        Err(_) => true,
    }
}
