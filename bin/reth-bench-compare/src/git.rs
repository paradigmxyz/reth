//! Git operations for branch management and compilation.

use eyre::{eyre, Result, WrapErr};
use std::process::{Command, Stdio};
use tracing::{debug, info, warn};

/// Manages git operations for branch switching and compilation
#[derive(Debug, Clone)]
pub struct GitManager {
    repo_root: String,
}

impl GitManager {
    /// Create a new GitManager, detecting the repository root
    pub fn new() -> Result<Self> {
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .wrap_err("Failed to execute git command - is git installed?")?;

        if !output.status.success() {
            return Err(eyre!("Not in a git repository or git command failed"));
        }

        let repo_root = String::from_utf8(output.stdout)
            .wrap_err("Git output is not valid UTF-8")?
            .trim()
            .to_string();

        info!("Detected git repository at: {}", repo_root);
        Ok(Self { repo_root })
    }

    /// Get the current git branch name
    pub fn get_current_branch(&self) -> Result<String> {
        let output = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to get current branch")?;

        if !output.status.success() {
            return Err(eyre!("Failed to determine current branch"));
        }

        let branch = String::from_utf8(output.stdout)
            .wrap_err("Branch name is not valid UTF-8")?
            .trim()
            .to_string();

        if branch.is_empty() {
            return Err(eyre!("Not on a named branch (detached HEAD?)"));
        }

        Ok(branch)
    }

    /// Check if the git working directory is clean
    pub fn validate_clean_state(&self) -> Result<()> {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to check git status")?;

        if !output.status.success() {
            return Err(eyre!("Git status command failed"));
        }

        let status_output =
            String::from_utf8(output.stdout).wrap_err("Git status output is not valid UTF-8")?;

        if !status_output.trim().is_empty() {
            warn!("Git working directory has uncommitted changes:");
            warn!("{}", status_output);
            return Err(eyre!(
                "Git working directory is not clean. Please commit or stash changes before running benchmark comparison."
            ));
        }

        info!("Git working directory is clean");
        Ok(())
    }

    /// Validate that the specified branches exist
    pub fn validate_branches(&self, branches: &[&str]) -> Result<()> {
        for &branch in branches {
            let output = Command::new("git")
                .args(["rev-parse", "--verify", &format!("refs/heads/{}", branch)])
                .current_dir(&self.repo_root)
                .output()
                .wrap_err_with(|| format!("Failed to validate branch '{}'", branch))?;

            if !output.status.success() {
                return Err(eyre!("Branch '{}' does not exist", branch));
            }

            info!("Validated branch exists: {}", branch);
        }

        Ok(())
    }

    /// Switch to the specified git branch
    pub fn switch_branch(&self, branch: &str) -> Result<()> {
        info!("Switching to branch: {}", branch);

        let output = Command::new("git")
            .args(["checkout", branch])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err_with(|| format!("Failed to switch to branch '{}'", branch))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!("Failed to switch to branch '{}': {}", branch, stderr));
        }

        // Verify we're on the correct branch
        let current = self.get_current_branch()?;
        if current != branch {
            return Err(eyre!(
                "Branch switch verification failed: expected '{}', got '{}'",
                branch,
                current
            ));
        }

        info!("Successfully switched to branch: {}", branch);
        Ok(())
    }

    /// Compile reth using `make profiling`
    pub fn compile_reth(&self) -> Result<()> {
        info!("Compiling reth with profiling configuration...");

        let output = Command::new("make")
            .arg("profiling")
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to execute make profiling command")?;

        // Print stdout and stderr with prefixes
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        for line in stdout.lines() {
            if !line.trim().is_empty() {
                println!("[MAKE] {}", line);
            }
        }

        for line in stderr.lines() {
            if !line.trim().is_empty() {
                eprintln!("[MAKE] {}", line);
            }
        }

        if !output.status.success() {
            return Err(eyre!("Compilation failed with exit code: {:?}", output.status.code()));
        }

        info!("Reth compilation completed successfully");
        Ok(())
    }

    /// Compile and install reth-bench using `make install-reth-bench`
    pub fn compile_reth_bench(&self) -> Result<()> {
        info!("Compiling and installing reth-bench...");

        let output = Command::new("make")
            .arg("install-reth-bench")
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to execute make install-reth-bench command")?;

        // Print stdout and stderr with prefixes
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        for line in stdout.lines() {
            if !line.trim().is_empty() {
                println!("[MAKE-BENCH] {}", line);
            }
        }

        for line in stderr.lines() {
            if !line.trim().is_empty() {
                eprintln!("[MAKE-BENCH] {}", line);
            }
        }

        if !output.status.success() {
            return Err(eyre!(
                "reth-bench compilation failed with exit code: {:?}",
                output.status.code()
            ));
        }

        info!("reth-bench compilation completed successfully");
        Ok(())
    }

    /// Get the latest commit hash for logging/reporting
    pub fn get_commit_hash(&self) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to get commit hash")?;

        if !output.status.success() {
            return Err(eyre!("Failed to get commit hash"));
        }

        let hash = String::from_utf8(output.stdout)
            .wrap_err("Commit hash is not valid UTF-8")?
            .trim()
            .to_string();

        Ok(hash)
    }

    /// Get the repository root path
    pub fn repo_root(&self) -> &str {
        &self.repo_root
    }

    /// Pull latest changes from remote (useful for ensuring branches are up-to-date)
    pub fn pull_latest(&self, branch: &str) -> Result<()> {
        info!("Pulling latest changes for branch: {}", branch);

        // First, fetch to make sure we have the latest refs
        let fetch_output = Command::new("git")
            .args(["fetch", "origin", branch])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to fetch latest changes")?;

        if !fetch_output.status.success() {
            let stderr = String::from_utf8_lossy(&fetch_output.stderr);
            warn!("Git fetch failed (continuing anyway): {}", stderr);
        }

        // Try to pull if we're on the right branch
        let current = self.get_current_branch()?;
        if current == branch {
            let pull_output = Command::new("git")
                .args(["pull", "origin", branch])
                .current_dir(&self.repo_root)
                .output()
                .wrap_err("Failed to pull latest changes")?;

            if !pull_output.status.success() {
                let stderr = String::from_utf8_lossy(&pull_output.stderr);
                warn!("Git pull failed (continuing anyway): {}", stderr);
            } else {
                info!("Successfully pulled latest changes");
            }
        }

        Ok(())
    }
}
