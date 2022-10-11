/// Wrapper around primitive U256 type to handle json parsing
pub mod jsonu256;

pub use jsonu256::JsonU256;

use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

pub(crate) fn find_all_json_tests(path: &Path) -> Vec<PathBuf> {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .map(DirEntry::into_path)
        .collect::<Vec<PathBuf>>()
}
