//! Compilation operations for reth and reth-bench.

use eyre::{eyre, Result, WrapErr};
use std::process::Command;
use tracing::{debug, error, info, warn};

/// Manages compilation operations for reth components
#[derive(Debug)]
pub struct CompilationManager {
    repo_root: String,
}

impl CompilationManager {
    /// Create a new CompilationManager
    pub fn new(repo_root: String) -> Self {
        Self { repo_root }
    }

    /// Compile reth using `make profiling`
    pub fn compile_reth(&self) -> Result<()> {
        info!("Compiling reth with profiling configuration...");

        let output = Command::new("make")
            .arg("profiling")
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to execute make profiling command")?;

        // Print stdout and stderr with prefixes at debug level
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        for line in stdout.lines() {
            if !line.trim().is_empty() {
                debug!("[MAKE] {}", line);
            }
        }

        for line in stderr.lines() {
            if !line.trim().is_empty() {
                debug!("[MAKE] {}", line);
            }
        }

        if !output.status.success() {
            // Print all output when compilation fails
            error!("Make profiling failed with exit code: {:?}", output.status.code());

            if !stdout.trim().is_empty() {
                error!("Make stdout:");
                for line in stdout.lines() {
                    error!("  {}", line);
                }
            }

            if !stderr.trim().is_empty() {
                error!("Make stderr:");
                for line in stderr.lines() {
                    error!("  {}", line);
                }
            }

            return Err(eyre!("Compilation failed with exit code: {:?}", output.status.code()));
        }

        info!("Reth compilation completed successfully");
        Ok(())
    }

    /// Check if reth-bench is available in PATH
    pub fn is_reth_bench_available(&self) -> bool {
        match Command::new("command").args(["-v", "reth-bench"]).output() {
            Ok(output) => {
                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout);
                    info!("Found reth-bench: {}", path);
                    true
                } else {
                    false
                }
            }
            Err(_) => false,
        }
    }

    /// Ensure reth-bench is available, compiling if necessary
    pub fn ensure_reth_bench_available(&self) -> Result<()> {
        if self.is_reth_bench_available() {
            info!("reth-bench is already available");
            Ok(())
        } else {
            warn!("reth-bench not found in PATH, compiling and installing...");
            self.compile_reth_bench()
        }
    }

    /// Compile and install reth-bench using `make install-reth-bench`
    pub fn compile_reth_bench(&self) -> Result<()> {
        info!("Compiling and installing reth-bench...");

        let output = Command::new("make")
            .arg("install-reth-bench")
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to execute make install-reth-bench command")?;

        // Print stdout and stderr with prefixes at debug level
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        for line in stdout.lines() {
            if !line.trim().is_empty() {
                debug!("[MAKE-BENCH] {}", line);
            }
        }

        for line in stderr.lines() {
            if !line.trim().is_empty() {
                debug!("[MAKE-BENCH] {}", line);
            }
        }

        if !output.status.success() {
            // Print all output when compilation fails
            error!("Make install-reth-bench failed with exit code: {:?}", output.status.code());

            if !stdout.trim().is_empty() {
                error!("Make stdout:");
                for line in stdout.lines() {
                    error!("  {}", line);
                }
            }

            if !stderr.trim().is_empty() {
                error!("Make stderr:");
                for line in stderr.lines() {
                    error!("  {}", line);
                }
            }

            return Err(eyre!(
                "reth-bench compilation failed with exit code: {:?}",
                output.status.code()
            ));
        }

        info!("reth-bench compilation completed successfully");
        Ok(())
    }
}
