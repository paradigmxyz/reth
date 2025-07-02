//! Compilation operations for reth and reth-bench.

use eyre::{eyre, Result, WrapErr};
use std::{fs, path::PathBuf, process::Command};
use tracing::{debug, error, info, warn};

/// Manages compilation operations for reth components
#[derive(Debug)]
pub struct CompilationManager {
    repo_root: String,
    output_dir: PathBuf,
}

impl CompilationManager {
    /// Create a new CompilationManager
    pub fn new(repo_root: String, output_dir: PathBuf) -> Self {
        Self { repo_root, output_dir }
    }

    /// Get the path for a cached binary based on git reference
    fn get_cached_binary_path(&self, git_ref: &str) -> PathBuf {
        let sanitized_ref = git_ref.replace('/', "-").replace('\\', "-");
        self.output_dir.join("bin").join(format!("reth_{}", sanitized_ref))
    }

    /// Check if a cached binary exists for the given git reference
    pub fn has_cached_binary(&self, git_ref: &str) -> bool {
        let binary_path = self.get_cached_binary_path(git_ref);
        binary_path.exists()
    }

    /// Get the path to the cached binary (for use by NodeManager)
    pub fn get_binary_path(&self, git_ref: &str) -> PathBuf {
        self.get_cached_binary_path(git_ref)
    }

    /// Compile reth using `make profiling` and cache the binary
    pub fn compile_reth(&self, git_ref: &str) -> Result<()> {
        // Check if we already have a cached binary
        let cached_path = self.get_cached_binary_path(git_ref);
        if cached_path.exists() {
            info!("Using cached binary for {}: {:?}", git_ref, cached_path);
            return Ok(());
        }

        info!("Compiling reth with profiling configuration for {}...", git_ref);

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

        // Copy the compiled binary to cache
        let source_path = PathBuf::from(&self.repo_root).join("target/profiling/reth");
        if !source_path.exists() {
            return Err(eyre!("Compiled binary not found at {:?}", source_path));
        }

        // Create bin directory if it doesn't exist
        let bin_dir = self.output_dir.join("bin");
        fs::create_dir_all(&bin_dir)
            .wrap_err("Failed to create bin directory")?;

        // Copy binary to cache
        fs::copy(&source_path, &cached_path)
            .wrap_err("Failed to copy binary to cache")?;

        // Make the cached binary executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&cached_path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&cached_path, perms)?;
        }

        info!("Cached compiled binary at: {:?}", cached_path);
        Ok(())
    }

    /// Check if reth-bench is available in PATH
    pub fn is_reth_bench_available(&self) -> bool {
        println!("output: {:?}", Command::new("command").args(["-v", "reth-bench"]).output());
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
