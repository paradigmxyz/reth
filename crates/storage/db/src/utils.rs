//! Utils crate for `db`.

use std::path::Path;

/// Check if a db is empty. It does not provide any information on the
/// validity of the data in it. We consider a database as non empty when it's a non empty directory.
pub fn is_database_empty<P: AsRef<Path>>(path: P) -> bool {
    let path = path.as_ref();

    if !path.exists() {
        true
    } else if path.is_file() {
        false
    } else if let Ok(dir) = path.read_dir() {
        dir.count() == 0
    } else {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_database_empty_false_if_db_path_is_a_file() {
        let db_file = tempfile::NamedTempFile::new().unwrap();

        let result = is_database_empty(&db_file);

        assert!(!result);
    }
}
