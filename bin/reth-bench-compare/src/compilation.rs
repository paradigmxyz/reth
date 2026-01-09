//! Compilation operations for reth and reth-bench.

use crate::git::GitManager;
use alloy_primitives::address;
use alloy_provider::{Provider, ProviderBuilder};
use eyre::{eyre, Result, WrapErr};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread::{self, JoinHandle},
};
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

    /// Detect if the RPC endpoint is an Optimism chain
    pub(crate) async fn detect_optimism_chain(&self, rpc_url: &str) -> Result<bool> {
        info!("Detecting chain type from RPC endpoint...");

        // Create Alloy provider
        let url = rpc_url.parse().map_err(|e| eyre!("Invalid RPC URL '{}': {}", rpc_url, e))?;
        let provider = ProviderBuilder::new().connect_http(url);

        // Check for Optimism predeploy at address 0x420000000000000000000000000000000000000F
        let is_optimism = !provider
            .get_code_at(address!("0x420000000000000000000000000000000000000F"))
            .await?
            .is_empty();

        if is_optimism {
            info!("Detected Optimism chain");
        } else {
            info!("Detected Ethereum chain");
        }

        Ok(is_optimism)
    }

    /// Get the path to the cached binary using explicit commit hash
    pub(crate) fn get_cached_binary_path_for_commit(
        &self,
        commit: &str,
        is_optimism: bool,
    ) -> PathBuf {
        let identifier = &commit[..8]; // Use first 8 chars of commit

        let binary_name = if is_optimism {
            format!("op-reth_{}", identifier)
        } else {
            format!("reth_{}", identifier)
        };

        self.output_dir.join("bin").join(binary_name)
    }

    /// Compile reth using cargo build and cache the binary
    pub(crate) fn compile_reth(
        &self,
        commit: &str,
        is_optimism: bool,
        features: &str,
        rustflags: &str,
    ) -> Result<()> {
        // Validate that current git commit matches the expected commit
        let current_commit = self.git_manager.get_current_commit()?;
        if current_commit != commit {
            return Err(eyre!(
                "Git commit mismatch! Expected: {}, but currently at: {}",
                &commit[..8],
                &current_commit[..8]
            ));
        }

        let cached_path = self.get_cached_binary_path_for_commit(commit, is_optimism);

        // Check if cached binary already exists (since path contains commit hash, it's valid)
        if cached_path.exists() {
            info!("Using cached binary (commit: {})", &commit[..8]);
            return Ok(());
        }

        info!("No cached binary found, compiling (commit: {})...", &commit[..8]);

        let binary_name = if is_optimism { "op-reth" } else { "reth" };

        info!(
            "Compiling {} with profiling configuration (commit: {})...",
            binary_name,
            &commit[..8]
        );

        let mut cmd = Command::new("cargo");
        cmd.arg("build").arg("--profile").arg("profiling");

        cmd.arg("--features").arg(features);
        info!("Using features: {features}");

        // Add bin-specific arguments for optimism
        if is_optimism {
            cmd.arg("--bin")
                .arg("op-reth")
                .arg("--manifest-path")
                .arg("crates/optimism/bin/Cargo.toml");
        }

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

    /// Create a git worktree for a specific commit at the given path
    fn create_worktree(&self, commit: &str, worktree_path: &PathBuf) -> Result<()> {
        if worktree_path.exists() {
            debug!("Removing existing worktree directory: {:?}", worktree_path);
            fs::remove_dir_all(worktree_path).wrap_err_with(|| {
                format!("Failed to remove existing worktree: {:?}", worktree_path)
            })?;
        }

        info!("Creating worktree at {:?} for commit {}", worktree_path, &commit[..8]);

        let output = Command::new("git")
            .args(["worktree", "add", "--detach", &worktree_path.to_string_lossy(), commit])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to execute git worktree add command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!("Failed to create git worktree: {}", stderr));
        }

        Ok(())
    }

    /// Remove a git worktree
    fn remove_worktree(&self, worktree_path: &PathBuf) -> Result<()> {
        debug!("Removing worktree at {:?}", worktree_path);

        let output = Command::new("git")
            .args(["worktree", "remove", "--force", &worktree_path.to_string_lossy()])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to execute git worktree remove command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to remove git worktree (may already be removed): {}", stderr);
        }

        if worktree_path.exists() {
            fs::remove_dir_all(worktree_path).wrap_err_with(|| {
                format!("Failed to remove worktree directory: {:?}", worktree_path)
            })?;
        }

        Ok(())
    }

    /// Compile reth in a specific worktree directory (for parallel compilation)
    fn compile_reth_in_worktree(
        worktree_path: &Path,
        commit: &str,
        is_optimism: bool,
        features: &str,
        rustflags: &str,
        output_dir: &Path,
        ref_type: &str,
    ) -> Result<()> {
        let binary_name = if is_optimism { "op-reth" } else { "reth" };

        info!(
            "[{}] Compiling {} with profiling configuration (commit: {})...",
            ref_type,
            binary_name,
            &commit[..8]
        );

        let mut cmd = Command::new("cargo");
        cmd.arg("build").arg("--profile").arg("profiling");

        cmd.arg("--features").arg(features);
        info!("[{}] Using features: {}", ref_type, features);

        if is_optimism {
            cmd.arg("--bin")
                .arg("op-reth")
                .arg("--manifest-path")
                .arg("crates/optimism/bin/Cargo.toml");
        }

        cmd.current_dir(worktree_path);
        cmd.env("RUSTFLAGS", rustflags);
        info!("[{}] Using RUSTFLAGS: {}", ref_type, rustflags);

        debug!("[{}] Compiling {} with {:?}", ref_type, binary_name, cmd);

        let output = cmd.output().wrap_err("Failed to execute cargo build command")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        for line in stdout.lines() {
            if !line.trim().is_empty() {
                debug!("[CARGO-{}] {}", ref_type.to_uppercase(), line);
            }
        }

        for line in stderr.lines() {
            if !line.trim().is_empty() {
                debug!("[CARGO-{}] {}", ref_type.to_uppercase(), line);
            }
        }

        if !output.status.success() {
            error!("[{}] Cargo build failed with exit code: {:?}", ref_type, output.status.code());

            if !stdout.trim().is_empty() {
                error!("[{}] Cargo stdout:", ref_type);
                for line in stdout.lines() {
                    error!("  {}", line);
                }
            }

            if !stderr.trim().is_empty() {
                error!("[{}] Cargo stderr:", ref_type);
                for line in stderr.lines() {
                    error!("  {}", line);
                }
            }

            return Err(eyre!(
                "[{}] Compilation failed with exit code: {:?}",
                ref_type,
                output.status.code()
            ));
        }

        info!("[{}] {} compilation completed", ref_type, binary_name);

        let source_path = worktree_path.join(format!("target/profiling/{}", binary_name));
        if !source_path.exists() {
            return Err(eyre!("[{}] Compiled binary not found at {:?}", ref_type, source_path));
        }

        let identifier = &commit[..8];
        let cached_binary_name = if is_optimism {
            format!("op-reth_{}", identifier)
        } else {
            format!("reth_{}", identifier)
        };
        let cached_path = output_dir.join("bin").join(cached_binary_name);

        let bin_dir = output_dir.join("bin");
        fs::create_dir_all(&bin_dir).wrap_err("Failed to create bin directory")?;

        fs::copy(&source_path, &cached_path).wrap_err("Failed to copy binary to cache")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&cached_path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&cached_path, perms)?;
        }

        info!("[{}] Cached compiled binary at: {:?}", ref_type, cached_path);
        Ok(())
    }

    /// Compile baseline and feature refs in parallel using git worktrees
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compile_reth_parallel(
        &self,
        baseline_commit: &str,
        feature_commit: &str,
        is_optimism: bool,
        baseline_features: &str,
        baseline_rustflags: &str,
        feature_features: &str,
        feature_rustflags: &str,
    ) -> Result<()> {
        let baseline_cached = self.get_cached_binary_path_for_commit(baseline_commit, is_optimism);
        let feature_cached = self.get_cached_binary_path_for_commit(feature_commit, is_optimism);

        let baseline_needs_compile = !baseline_cached.exists();
        let feature_needs_compile = !feature_cached.exists();

        if !baseline_needs_compile && !feature_needs_compile {
            info!(
                "Both binaries already cached (baseline: {}, feature: {})",
                &baseline_commit[..8],
                &feature_commit[..8]
            );
            return Ok(());
        }

        if !baseline_needs_compile {
            info!("Baseline binary already cached (commit: {})", &baseline_commit[..8]);
        }
        if !feature_needs_compile {
            info!("Feature binary already cached (commit: {})", &feature_commit[..8]);
        }

        let worktrees_dir = self.output_dir.join("worktrees");
        fs::create_dir_all(&worktrees_dir).wrap_err("Failed to create worktrees directory")?;

        let baseline_worktree = worktrees_dir.join("baseline");
        let feature_worktree = worktrees_dir.join("feature");

        if baseline_needs_compile {
            self.create_worktree(baseline_commit, &baseline_worktree)?;
        }
        if feature_needs_compile {
            self.create_worktree(feature_commit, &feature_worktree)?;
        }

        let cleanup = |manager: &Self| {
            if baseline_worktree.exists() {
                let _ = manager.remove_worktree(&baseline_worktree);
            }
            if feature_worktree.exists() {
                let _ = manager.remove_worktree(&feature_worktree);
            }
        };

        let (tx, rx) = mpsc::channel::<Result<()>>();

        let mut handles: Vec<JoinHandle<()>> = Vec::new();

        if baseline_needs_compile {
            let tx_baseline = tx.clone();
            let baseline_worktree_clone = baseline_worktree.clone();
            let baseline_commit_clone = baseline_commit.to_string();
            let baseline_features_clone = baseline_features.to_string();
            let baseline_rustflags_clone = baseline_rustflags.to_string();
            let output_dir_clone = self.output_dir.clone();

            let handle = thread::spawn(move || {
                let result = Self::compile_reth_in_worktree(
                    &baseline_worktree_clone,
                    &baseline_commit_clone,
                    is_optimism,
                    &baseline_features_clone,
                    &baseline_rustflags_clone,
                    &output_dir_clone,
                    "baseline",
                );
                let _ = tx_baseline.send(result);
            });
            handles.push(handle);
        }

        if feature_needs_compile {
            let tx_feature = tx.clone();
            let feature_worktree_clone = feature_worktree.clone();
            let feature_commit_clone = feature_commit.to_string();
            let feature_features_clone = feature_features.to_string();
            let feature_rustflags_clone = feature_rustflags.to_string();
            let output_dir_clone = self.output_dir.clone();

            let handle = thread::spawn(move || {
                let result = Self::compile_reth_in_worktree(
                    &feature_worktree_clone,
                    &feature_commit_clone,
                    is_optimism,
                    &feature_features_clone,
                    &feature_rustflags_clone,
                    &output_dir_clone,
                    "feature",
                );
                let _ = tx_feature.send(result);
            });
            handles.push(handle);
        }

        drop(tx);

        let mut errors = Vec::new();
        for result in rx {
            if let Err(e) = result {
                errors.push(e);
            }
        }

        for handle in handles {
            let _ = handle.join();
        }

        cleanup(self);

        if !errors.is_empty() {
            let error_msgs: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
            return Err(eyre!("Parallel compilation failed:\n{}", error_msgs.join("\n")));
        }

        info!("Parallel compilation completed successfully");
        Ok(())
    }
}
