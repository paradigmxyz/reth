//! Reproducible benchmark fixtures: the per-block `BlockAccessedState` captured
//! from a live node run.
//!
//! The cache-window benchmark only depends on *what state each block accessed* —
//! not on the EVM, the historical DB, or the Merkle proofs. Capturing the
//! `BlockAccessedState` for a fixed range of blocks therefore yields a portable,
//! node-independent dataset: replaying it through a `NetworkStateCache` is fully
//! deterministic, so the only variable left is the cache policy / window.
//!
//! Capturing raw blocks would NOT be reproducible — re-execution needs the parent
//! historical state present in the node DB at that exact height. The accessed-state
//! snapshot is the faithful, self-contained artifact.

use crate::accessed_state::BlockAccessedState;
use alloy_primitives::B256;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// One block's captured access-set, plus enough identity to order and cross-check it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccessedStateFixture {
    /// Canonical block number.
    pub block_number: u64,
    /// Canonical block hash.
    pub block_hash: B256,
    /// Parent block's state root (the anchor a witness would be proven against).
    pub parent_state_root: B256,
    /// State accessed while executing this block.
    pub accessed: BlockAccessedState,
}

impl AccessedStateFixture {
    /// File name used for this fixture within a capture directory.
    pub fn file_name(&self) -> String {
        format!("accessed_{:012}.bin", self.block_number)
    }
}

/// Serialize and write a single fixture into `dir` (created if missing).
///
/// Writes atomically via a temp file so a crashed capture run never leaves a
/// half-written fixture that would later deserialize into garbage.
pub fn save_fixture(dir: &Path, fixture: &AccessedStateFixture) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let encoded = bincode::serialize(fixture)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    let path = dir.join(fixture.file_name());
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &encoded)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Load every `accessed_*.bin` fixture in `dir`, sorted by ascending block number.
///
/// Fixtures that fail to deserialize are skipped (with the error returned in the
/// `skipped` list) rather than aborting the whole load.
pub fn load_fixtures(dir: &Path) -> io::Result<LoadedFixtures> {
    let mut fixtures = Vec::new();
    let mut skipped = Vec::new();

    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let is_fixture = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("accessed_") && n.ends_with(".bin"));
        if !is_fixture {
            continue;
        }
        match fs::read(&path).map_err(|e| e.to_string()).and_then(|bytes| {
            bincode::deserialize::<AccessedStateFixture>(&bytes).map_err(|e| e.to_string())
        }) {
            Ok(fixture) => fixtures.push(fixture),
            Err(err) => skipped.push((path, err)),
        }
    }

    fixtures.sort_by_key(|f| f.block_number);
    Ok(LoadedFixtures { fixtures, skipped })
}

/// Result of [`load_fixtures`].
#[derive(Debug, Default)]
pub struct LoadedFixtures {
    /// Successfully loaded fixtures, ascending by block number.
    pub fixtures: Vec<AccessedStateFixture>,
    /// Files that looked like fixtures but failed to load, with the reason.
    pub skipped: Vec<(PathBuf, String)>,
}

impl LoadedFixtures {
    /// True if the loaded block numbers form a gap-free contiguous range.
    ///
    /// A gap means the cache replay would see a discontinuity that never happens
    /// on a live chain (the LastN window is measured in block height), so the
    /// benchmark should warn about it.
    pub fn is_contiguous(&self) -> bool {
        self.fixtures
            .windows(2)
            .all(|w| w[1].block_number == w[0].block_number + 1)
    }

    /// Inclusive `(first, last)` block numbers, if any fixtures loaded.
    pub fn range(&self) -> Option<(u64, u64)> {
        match (self.fixtures.first(), self.fixtures.last()) {
            (Some(a), Some(b)) => Some((a.block_number, b.block_number)),
            _ => None,
        }
    }
}
