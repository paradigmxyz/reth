//! Compilation operations for reth and reth-bench.

use crate::git::{sanitize_git_ref, GitManager};
use eyre::{eyre, Result, WrapErr};
use std::{fs, path::PathBuf, process::Command};
use tracing::{debug, error, info, warn};

/// Manages compilation operations for reth components
#[derive(Debug)]
pub struct CompilationManager {
    repo_root: String,
    output_dir: PathBuf,
    git_manager: GitManager,
}

impl CompilationManager {
    /// Create a new CompilationManager
    pub fn new(repo_root: String, output_dir: PathBuf, git_manager: GitManager) -> Result<Self> {
        Ok(Self { repo_root, output_dir, git_manager })
    }

    /// Get the path to the cached binary
    pub fn get_cached_binary_path(&self, git_ref: &str) -> PathBuf {
        self.output_dir.join("bin").join(format!("reth_{}", sanitize_git_ref(git_ref)))
    }

    /// Check if a cached binary's commit matches the current git commit
    fn check_cached_binary_commit(&self, binary_path: &PathBuf) -> Result<Option<String>> {
        if !binary_path.exists() {
            return Ok(None);
        }

        // Run the binary with --version
        let output = Command::new(binary_path)
            .arg("--version")
            .output()
            .wrap_err("Failed to execute cached binary with --version")?;

        if !output.status.success() {
            warn!("Cached binary failed to run --version, will recompile");
            return Ok(None);
        }

        let version_output = String::from_utf8_lossy(&output.stdout);

        // Parse the commit SHA from the version output
        // Looking for line: "Commit SHA: 30110bca049a7d50ca53c6378e693287bcddaf5a"
        for line in version_output.lines() {
            if let Some(sha) = line.strip_prefix("Commit SHA: ") {
                return Ok(Some(sha.trim().to_string()));
            }
        }

        warn!("Could not find commit SHA in cached binary version output");
        Ok(None)
    }

    /// Compile reth using `make profiling` and cache the binary
    pub fn compile_reth(&self, git_ref: &str) -> Result<()> {
        let cached_path = self.get_cached_binary_path(git_ref);

        // Check if we have a cached binary with matching commit
        if let Some(cached_commit) = self.check_cached_binary_commit(&cached_path)? {
            let current_commit = self.git_manager.get_current_commit()?;

            if cached_commit == current_commit {
                info!("Using cached binary for {} (commit: {})", git_ref, &cached_commit[..8]);
                return Ok(());
            } else {
                info!(
                    "Cached binary commit mismatch for {} (cached: {}, current: {})",
                    git_ref,
                    &cached_commit[..8],
                    &current_commit[..8]
                );
                info!("Recompiling...");
            }
        } else {
            info!("No valid cached binary found for {}, compiling...", git_ref);
        }

        info!("Compiling reth with profiling configuration for {}...", git_ref);

        let mut cmd = Command::new("make");
        cmd.arg("profiling").current_dir(&self.repo_root);

        // Debug log the command
        debug!("Executing make command: {:?}", cmd);

        let output = cmd.output().wrap_err("Failed to execute make profiling command")?;

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

        info!("Reth compilation completed");

        // Copy the compiled binary to cache
        let source_path = PathBuf::from(&self.repo_root).join("target/profiling/reth");
        if !source_path.exists() {
            return Err(eyre!("Compiled binary not found at {:?}", source_path));
        }

        // Create bin directory if it doesn't exist
        let bin_dir = self.output_dir.join("bin");
        fs::create_dir_all(&bin_dir).wrap_err("Failed to create bin directory")?;

        // Copy binary to cache
        fs::copy(&source_path, &cached_path).wrap_err("Failed to copy binary to cache")?;

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
        match Command::new("which").arg("reth-bench").output() {
            Ok(output) => {
                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout);
                    info!("Found reth-bench: {}", path.trim());
                    true
                } else {
                    false
                }
            }
            Err(_) => false,
        }
    }

    /// Check if samply is available in PATH
    pub fn is_samply_available(&self) -> bool {
        match Command::new("which").arg("samply").output() {
            Ok(output) => {
                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout);
                    info!("Found samply: {}", path.trim());
                    true
                } else {
                    false
                }
            }
            Err(_) => false,
        }
    }

    /// Install samply using cargo
    pub fn install_samply(&self) -> Result<()> {
        info!("Installing samply via cargo...");

        let mut cmd = Command::new("cargo");
        cmd.args(["install", "--locked", "samply"]);

        // Debug log the command
        debug!("Executing cargo command: {:?}", cmd);

        let output = cmd.output().wrap_err("Failed to execute cargo install samply command")?;

        // Print stdout and stderr with prefixes at debug level
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        for line in stdout.lines() {
            if !line.trim().is_empty() {
                debug!("[CARGO-SAMPLY] {}", line);
            }
        }

        for line in stderr.lines() {
            if !line.trim().is_empty() {
                debug!("[CARGO-SAMPLY] {}", line);
            }
        }

        if !output.status.success() {
            // Print all output when installation fails
            error!("Cargo install samply failed with exit code: {:?}", output.status.code());

            if !stdout.trim().is_empty() {
                error!("Cargo stdout:");
                for line in stdout.lines() {
                    error!("  {}", line);
                }
            }

            if !stderr.trim().is_empty() {
                error!("Cargo stderr:");
                for line in stderr.lines() {
                    error!("  {}", line);
                }
            }

            return Err(eyre!(
                "samply installation failed with exit code: {:?}",
                output.status.code()
            ));
        }

        info!("Samply installation completed");
        Ok(())
    }

    /// Ensure samply is available, installing if necessary
    pub fn ensure_samply_available(&self) -> Result<()> {
        if self.is_samply_available() {
            Ok(())
        } else {
            warn!("samply not found in PATH, installing...");
            self.install_samply()
        }
    }

    /// Ensure reth-bench is available, compiling if necessary
    pub fn ensure_reth_bench_available(&self) -> Result<()> {
        if self.is_reth_bench_available() {
            Ok(())
        } else {
            warn!("reth-bench not found in PATH, compiling and installing...");
            self.compile_reth_bench()
        }
    }

    /// Compile and install reth-bench using `make install-reth-bench`
    pub fn compile_reth_bench(&self) -> Result<()> {
        info!("Compiling and installing reth-bench...");

        let mut cmd = Command::new("make");
        cmd.arg("install-reth-bench").current_dir(&self.repo_root);

        // Debug log the command
        debug!("Executing make command: {:?}", cmd);

        let output = cmd.output().wrap_err("Failed to execute make install-reth-bench command")?;

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

        info!("Reth-bench compilation completed");
        Ok(())
    }
}
