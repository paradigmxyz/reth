use clap::Parser;
use reth_db::mdbx::{self, ffi};
use std::path::PathBuf;

/// Copies the MDBX database to a new location.
///
/// Equivalent to the standalone `mdbx_copy` tool but bundled into reth.
#[derive(Parser, Debug)]
pub struct Command {
    /// Destination path for the database copy.
    dest: PathBuf,

    /// Compact the database while copying (reclaims free space).
    #[arg(short, long)]
    compact: bool,

    /// Force dynamic size for the destination database.
    #[arg(short = 'd', long)]
    force_dynamic_size: bool,

    /// Throttle to avoid MVCC pressure on writers.
    #[arg(short = 'p', long)]
    throttle_mvcc: bool,
}

impl Command {
    /// Execute `db copy` command
    pub fn execute(self, db: &mdbx::DatabaseEnv) -> eyre::Result<()> {
        let mut flags: ffi::MDBX_copy_flags_t = ffi::MDBX_CP_DEFAULTS;
        if self.compact {
            flags |= ffi::MDBX_CP_COMPACT;
        }
        if self.force_dynamic_size {
            flags |= ffi::MDBX_CP_FORCE_DYNAMIC_SIZE;
        }
        if self.throttle_mvcc {
            flags |= ffi::MDBX_CP_THROTTLE_MVCC;
        }

        let dest = self
            .dest
            .to_str()
            .ok_or_else(|| eyre::eyre!("destination path must be valid UTF-8"))?;
        let dest_cstr = std::ffi::CString::new(dest)?;

        println!("Copying database to {} ...", self.dest.display());

        let rc = db.with_raw_env_ptr(|env_ptr| unsafe {
            ffi::mdbx_env_copy(env_ptr, dest_cstr.as_ptr(), flags)
        });

        if rc != 0 {
            eyre::bail!("mdbx_env_copy failed with error code {rc}: {}", unsafe {
                std::ffi::CStr::from_ptr(ffi::mdbx_strerror(rc)).to_string_lossy()
            });
        }

        println!("Done.");
        Ok(())
    }
}
