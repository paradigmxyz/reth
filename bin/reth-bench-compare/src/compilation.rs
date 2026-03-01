//! Compilation operations for reth and reth-bench.

use crate::git::GitManager;
use eyre::{eyre, Result, WrapErr};
use std::{fs, path::PathBuf, process::Command};
use tracing::{debug, error, info, warn};

/// Manages compilation operations for reth components
#[derive(Debug)]
pub(crate) struct CompilationManager {
    repo_root: String,
    output_dir: PathBuf,
    git_manager: GitManager,
}

impl CompilationManager {
    /// Create a new `CompilationManager`
    pub(crate) const fn new(
        repo_root: String,
        output_dir: PathBuf,
        git_manager: GitManager,
    ) -> Result<Self> {
        Ok(Self { repo_root, output_dir, git_manager })
    }

    /// Get the path to the cached binary using explicit commit hash
    pub(crate) fn get_cached_binary_path_for_commit(&self, commit: &str) -> PathBuf {
        let identifier = &commit[..8]; // Use first 8 chars of commit
        self.output_dir.join("bin").join(format!("reth_{identifier}"))
    }

    /// Compile reth using cargo build and cache the binary
    pub(crate) fn compile_reth(&self, commit: &str, features: &str, rustflags: &str) -> Result<()> {
        // Validate that current git commit matches the expected commit
        let current_commit = self.git_manager.get_current_commit()?;
        if current_commit != commit {
            return Err(eyre!(
                "Git commit mismatch! Expected: {}, but currently at: {}",
                &commit[..8],
                &current_commit[..8]
            ));
        }

        let cached_path = self.get_cached_binary_path_for_commit(commit);

        // Check if cached binary already exists (since path contains commit hash, it's valid)
        if cached_path.exists() {
            info!("Using cached binary (commit: {})", &commit[..8]);
            return Ok(());
        }

        info!("No cached binary found, compiling (commit: {})...", &commit[..8]);

        let binary_name = "reth";

        info!(
            "Compiling {} with profiling configuration (commit: {})...",
            binary_name,
            &commit[..8]
        );

        let mut cmd = Command::new("cargo");
        cmd.arg("build").arg("--profile").arg("profiling");

        cmd.arg("--features").arg(features);
        info!("Using features: {features}");

        cmd.current_dir(&self.repo_root);

        // Set RUSTFLAGS
        cmd.env("RUSTFLAGS", rustflags);
        info!("Using RUSTFLAGS: {rustflags}");

        info!("Compiling {binary_name} with {cmd:?}");

        let output = cmd.output().wrap_err("Failed to execute cargo build command")?;

        // Print stdout and stderr with prefixes at debug level
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        for line in stdout.lines() {
            if !line.trim().is_empty() {
                debug!("[CARGO] {}", line);
            }
        }

        for line in stderr.lines() {
            if !line.trim().is_empty() {
                debug!("[CARGO] {}", line);
            }
        }

        if !output.status.success() {
            // Print all output when compilation fails
            error!("Cargo build failed with exit code: {:?}", output.status.code());

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

            return Err(eyre!("Compilation failed with exit code: {:?}", output.status.code()));
        }

        info!("{} compilation completed", binary_name);

        // Copy the compiled binary to cache
        let source_path =
            PathBuf::from(&self.repo_root).join(format!("target/profiling/{}", binary_name));
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
    pub(crate) fn is_reth_bench_available(&self) -> bool {
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
    pub(crate) fn is_samply_available(&self) -> bool {
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
    pub(crate) fn install_samply(&self) -> Result<()> {
        info!("Installing samply via cargo...");

        let mut cmd = Command::new("cargo");
        cmd.args(["install", "--locked", "samply"]);

        info!("Installing samply with {cmd:?}");

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
    pub(crate) fn ensure_samply_available(&self) -> Result<()> {
        if self.is_samply_available() {
            Ok(())
        } else {
            warn!("samply not found in PATH, installing...");
            self.install_samply()
        }
    }

    /// Ensure reth-bench is available, compiling if necessary
    pub(crate) fn ensure_reth_bench_available(&self) -> Result<()> {
        if self.is_reth_bench_available() {
            Ok(())
        } else {
            warn!("reth-bench not found in PATH, compiling and installing...");
            self.compile_reth_bench()
        }
    }

    /// Compile and install reth-bench using `make install-reth-bench`
    pub(crate) fn compile_reth_bench(&self) -> Result<()> {
        info!("Compiling and installing reth-bench...");

        let mut cmd = Command::new("make");
        cmd.arg("install-reth-bench").current_dir(&self.repo_root);

        info!("Compiling reth-bench with {cmd:?}");

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
