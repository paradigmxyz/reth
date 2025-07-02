//! Git operations for branch management.

use eyre::{eyre, Result, WrapErr};
use std::process::Command;
use tracing::{info, warn};

/// Manages git operations for branch switching
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

    /// Fetch all refs from remote to ensure we have latest branches and tags
    pub fn fetch_all(&self) -> Result<()> {
        info!("Fetching latest refs from remote...");

        let output = Command::new("git")
            .args(["fetch", "--all", "--tags", "--quiet", "--force"])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to fetch latest refs")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Only warn if there's actual error content, not just fetch progress
            if !stderr.trim().is_empty() && !stderr.contains("-> origin/") {
                warn!("Git fetch encountered issues (continuing anyway): {}", stderr);
            }
        } else {
            info!("Successfully fetched latest refs");
        }

        Ok(())
    }

    /// Validate that the specified git references exist (branches, tags, or commits)
    pub fn validate_refs(&self, refs: &[&str]) -> Result<()> {
        for &git_ref in refs {
            // Try branch first, then tag, then commit
            let branch_check = Command::new("git")
                .args(["rev-parse", "--verify", &format!("refs/heads/{git_ref}")])
                .current_dir(&self.repo_root)
                .output();

            let tag_check = Command::new("git")
                .args(["rev-parse", "--verify", &format!("refs/tags/{git_ref}")])
                .current_dir(&self.repo_root)
                .output();

            let commit_check = Command::new("git")
                .args(["rev-parse", "--verify", &format!("{git_ref}^{{commit}}")])
                .current_dir(&self.repo_root)
                .output();

            let mut found = false;

            if let Ok(output) = branch_check {
                if output.status.success() {
                    info!("Validated branch exists: {}", git_ref);
                    found = true;
                }
            }

            if !found {
                if let Ok(output) = tag_check {
                    if output.status.success() {
                        info!("Validated tag exists: {}", git_ref);
                        found = true;
                    }
                }
            }

            if !found {
                if let Ok(output) = commit_check {
                    if output.status.success() {
                        info!("Validated commit exists: {}", git_ref);
                        found = true;
                    }
                }
            }

            if !found {
                return Err(eyre!(
                    "Git reference '{}' does not exist as branch, tag, or commit",
                    git_ref
                ));
            }
        }

        Ok(())
    }

    /// Switch to the specified git reference (branch, tag, or commit)
    pub fn switch_ref(&self, git_ref: &str) -> Result<()> {
        info!("Switching to git reference: {}", git_ref);

        let output = Command::new("git")
            .args(["checkout", git_ref])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err_with(|| format!("Failed to switch to reference '{git_ref}'"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!("Failed to switch to reference '{}': {}", git_ref, stderr));
        }

        // For tags, we might be in detached HEAD state, which is fine for benchmarking
        // Just verify the checkout succeeded by checking the current commit
        let current_commit_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to get current commit")?;

        if !current_commit_output.status.success() {
            return Err(eyre!("Failed to verify git checkout"));
        }

        info!("Successfully switched to reference: {}", git_ref);
        Ok(())
    }

    /// Switch to the specified git branch (for restoration after benchmarking)
    pub fn switch_branch(&self, branch: &str) -> Result<()> {
        info!("Switching to branch: {}", branch);

        let output = Command::new("git")
            .args(["checkout", branch])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err_with(|| format!("Failed to switch to branch '{branch}'"))?;

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

    /// Get the current commit hash
    pub fn get_current_commit(&self) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to get current commit")?;

        if !output.status.success() {
            return Err(eyre!("Failed to get current commit hash"));
        }

        let commit_hash = String::from_utf8(output.stdout)
            .wrap_err("Commit hash is not valid UTF-8")?
            .trim()
            .to_string();

        Ok(commit_hash)
    }

    /// Get the repository root path
    pub fn repo_root(&self) -> &str {
        &self.repo_root
    }
}

/// Sanitize a git reference for use in file names.
pub fn sanitize_git_ref(git_ref: &str) -> String {
    git_ref.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "-")
}
