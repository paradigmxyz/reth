use alloy_primitives::B256;
use partial_stateless::PartialStatelessSidecar;
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

pub(crate) fn sidecar_path(sidecar_dir: &Path, block_number: u64, block_hash: B256) -> PathBuf {
    sidecar_dir.join(format!("block_{}_{:?}.bin", block_number, block_hash))
}

pub(crate) fn read_sidecar(
    path: &Path,
    wait: Duration,
) -> eyre::Result<(Vec<u8>, PartialStatelessSidecar)> {
    let start = Instant::now();
    loop {
        match fs::read(path) {
            Ok(bytes) => match bincode::deserialize(&bytes) {
                Ok(sidecar) => return Ok((bytes, sidecar)),
                Err(_) if start.elapsed() < wait => {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(err) => {
                    return Err(eyre::eyre!("failed to decode sidecar {}: {err}", path.display()));
                }
            },
            Err(err) if err.kind() == ErrorKind::NotFound && start.elapsed() < wait => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(err) => {
                return Err(eyre::eyre!("failed to read sidecar {}: {err}", path.display()));
            }
        }
    }
}
