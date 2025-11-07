//! Git operations for branch management.

use eyre::{eyre, Result, WrapErr};
use std::process::Command;
use tracing::{info, warn};

/// Manages git operations for branch switching
#[derive(Debug, Clone)]
pub(crate) struct GitManager {
    repo_root: String,
}

impl GitManager {
    /// Create a new `GitManager`, detecting the repository root
    pub(crate) fn new() -> Result<Self> {
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

        let manager = Self { repo_root };
        info!(
            "Detected git repository at: {}, current reference: {}",
            manager.repo_root(),
            manager.get_current_ref()?
        );

        Ok(manager)
    }

    /// Get the current git branch name
    pub(crate) fn get_current_branch(&self) -> Result<String> {
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

    /// Get the current git reference (branch name, tag, or commit hash)
    pub(crate) fn get_current_ref(&self) -> Result<String> {
        // First try to get branch name
        if let Ok(branch) = self.get_current_branch() {
            return Ok(branch);
        }

        // If not on a branch, check if we're on a tag
        let tag_output = Command::new("git")
            .args(["describe", "--exact-match", "--tags", "HEAD"])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to check for tag")?;

        if tag_output.status.success() {
            let tag = String::from_utf8(tag_output.stdout)
                .wrap_err("Tag name is not valid UTF-8")?
                .trim()
                .to_string();
            return Ok(tag);
        }

        // If not on a branch or tag, return the commit hash
        let commit_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to get current commit")?;

        if !commit_output.status.success() {
            return Err(eyre!("Failed to get current commit hash"));
        }

        let commit_hash = String::from_utf8(commit_output.stdout)
            .wrap_err("Commit hash is not valid UTF-8")?
            .trim()
            .to_string();

        Ok(commit_hash)
    }

    /// Check if the git working directory has uncommitted changes to tracked files
    pub(crate) fn validate_clean_state(&self) -> Result<()> {
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

        // Check for uncommitted changes to tracked files
        // Status codes: M = modified, A = added, D = deleted, R = renamed, C = copied, U = updated
        // ?? = untracked files (we want to ignore these)
        let has_uncommitted_changes = status_output.lines().any(|line| {
            if line.len() >= 2 {
                let status = &line[0..2];
                // Ignore untracked files (??) and ignored files (!!)
                !matches!(status, "??" | "!!")
            } else {
                false
            }
        });

        if has_uncommitted_changes {
            warn!("Git working directory has uncommitted changes to tracked files:");
            for line in status_output.lines() {
                if line.len() >= 2 && !matches!(&line[0..2], "??" | "!!") {
                    warn!("  {}", line);
                }
            }
            return Err(eyre!(
                "Git working directory has uncommitted changes to tracked files. Please commit or stash changes before running benchmark comparison."
            ));
        }

        // Check if there are untracked files and log them as info
        let untracked_files: Vec<&str> =
            status_output.lines().filter(|line| line.starts_with("??")).collect();

        if !untracked_files.is_empty() {
            info!(
                "Git working directory has {} untracked files (this is OK)",
                untracked_files.len()
            );
        }

        info!("Git working directory is clean (no uncommitted changes to tracked files)");
        Ok(())
    }

    /// Fetch all refs from remote to ensure we have latest branches and tags
    pub(crate) fn fetch_all(&self) -> Result<()> {
        let output = Command::new("git")
            .args(["fetch", "--all", "--tags", "--quiet", "--force"])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to fetch latest refs")?;

        if output.status.success() {
            info!("Fetched latest refs");
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Only warn if there's actual error content, not just fetch progress
            if !stderr.trim().is_empty() && !stderr.contains("-> origin/") {
                warn!("Git fetch encountered issues (continuing anyway): {}", stderr);
            }
        }

        Ok(())
    }

    /// Validate that the specified git references exist (branches, tags, or commits)
    pub(crate) fn validate_refs(&self, refs: &[&str]) -> Result<()> {
        for &git_ref in refs {
            // Try to resolve the ref similar to `git checkout` by peeling to a commit.
            // First try the ref as-is with ^{commit}, then fall back to origin/{ref}^{commit}.
            let as_is = format!("{git_ref}^{{commit}}");
            let ref_check = Command::new("git")
                .args(["rev-parse", "--verify", &as_is])
                .current_dir(&self.repo_root)
                .output();

            let found = if let Ok(output) = ref_check
                && output.status.success()
            {
                info!("Validated reference exists: {}", git_ref);
                true
            } else {
                // Try remote-only branches via origin/{ref}
                let origin_ref = format!("origin/{git_ref}^{{commit}}");
                let origin_check = Command::new("git")
                    .args(["rev-parse", "--verify", &origin_ref])
                    .current_dir(&self.repo_root)
                    .output();

                if let Ok(output) = origin_check
                    && output.status.success()
                {
                    info!("Validated remote reference exists: origin/{}", git_ref);
                    true
                } else {
                    false
                }
            };

            if !found {
                return Err(eyre!(
                    "Git reference '{}' does not exist as branch, tag, or commit (tried '{}' and 'origin/{}^{{commit}}')",
                    git_ref,
                    format!("{git_ref}^{{commit}}"),
                    git_ref,
                ));
            }
        }

        Ok(())
    }

    /// Switch to the specified git reference (branch, tag, or commit)
    pub(crate) fn switch_ref(&self, git_ref: &str) -> Result<()> {
        // First checkout the reference
        let output = Command::new("git")
            .args(["checkout", git_ref])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err_with(|| format!("Failed to switch to reference '{git_ref}'"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!("Failed to switch to reference '{}': {}", git_ref, stderr));
        }

        // Check if this is a branch that tracks a remote and pull latest changes
        let is_branch = Command::new("git")
            .args(["show-ref", "--verify", "--quiet", &format!("refs/heads/{git_ref}")])
            .current_dir(&self.repo_root)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if is_branch {
            // Check if the branch tracks a remote
            let tracking_output = Command::new("git")
                .args([
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    &format!("{git_ref}@{{upstream}}"),
                ])
                .current_dir(&self.repo_root)
                .output();

            if let Ok(output) = tracking_output
                && output.status.success()
            {
                let upstream = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !upstream.is_empty() && upstream != format!("{git_ref}@{{upstream}}") {
                    // Branch tracks a remote, pull latest changes
                    info!("Pulling latest changes for branch: {}", git_ref);

                    let pull_output = Command::new("git")
                        .args(["pull", "--ff-only"])
                        .current_dir(&self.repo_root)
                        .output()
                        .wrap_err_with(|| {
                            format!("Failed to pull latest changes for branch '{git_ref}'")
                        })?;

                    if pull_output.status.success() {
                        info!("Successfully pulled latest changes for branch: {}", git_ref);
                    } else {
                        let stderr = String::from_utf8_lossy(&pull_output.stderr);
                        warn!("Failed to pull latest changes for branch '{}': {}", git_ref, stderr);
                        // Continue anyway, we'll use whatever version we have
                    }
                }
            }
        }

        // Verify the checkout succeeded by checking the current commit
        let current_commit_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.repo_root)
            .output()
            .wrap_err("Failed to get current commit")?;

        if !current_commit_output.status.success() {
            return Err(eyre!("Failed to verify git checkout"));
        }

        info!("Switched to reference: {}", git_ref);
        Ok(())
    }

    /// Get the current commit hash
    pub(crate) fn get_current_commit(&self) -> Result<String> {
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
    pub(crate) fn repo_root(&self) -> &str {
        &self.repo_root
    }
}

#[cfg(test)]
mod tests {
    use super::GitManager;
    use eyre::{eyre, Result};
    use std::{fs, path::Path, process::Command};
    use tempfile::TempDir;

    fn run_git(dir: &Path, args: &[&str]) -> Result<()> {
        let out = Command::new("git").args(args).current_dir(dir).output()?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(eyre!("git {:?} failed: {}", args, stderr));
        }
        Ok(())
    }

    fn git_out(dir: &Path, args: &[&str]) -> Result<String> {
        let out = Command::new("git").args(args).current_dir(dir).output()?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(eyre!("git {:?} failed: {}", args, stderr));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    // Creates a bare origin and a working clone; returns (origin, work)
    fn setup_repo() -> Result<(TempDir, std::path::PathBuf, std::path::PathBuf)> {
        let base = TempDir::new()?;
        let base_path = base.path().to_path_buf();
        let origin_path = base_path.join("origin.git");
        // git init --bare <origin>
        run_git(&base_path, &["init", "--bare", origin_path.to_str().unwrap()])?;

        // git clone <origin> work
        run_git(&base_path, &["clone", origin_path.to_str().unwrap(), "work"])?;
        let work_path = base_path.join("work");

        // Configure identity
        run_git(&work_path, &["config", "user.email", "test@example.com"])?;
        run_git(&work_path, &["config", "user.name", "Test User"])?;

        // Initial commit on default branch
        fs::write(work_path.join("README.md"), "init\n")?;
        run_git(&work_path, &["add", "."])?;
        run_git(&work_path, &["commit", "-m", "init"])?;

        // Remember the current branch name (main or master)
        let _ = git_out(&work_path, &["rev-parse", "--abbrev-ref", "HEAD"]); // ensure branch exists

        // Push base branch
        run_git(&work_path, &["push", "-u", "origin", "HEAD"])?;

        Ok((base, origin_path, work_path))
    }

    #[test]
    fn validate_remote_only_branch_succeeds() -> Result<()> {
        let (_tmp, _origin, work) = setup_repo()?;

        // Create a branch, push it, then delete the local branch to make it remote-only
        run_git(&work, &["checkout", "-b", "yk/compute_trie2"])?;
        fs::write(work.join("feature.txt"), "feature\n")?;
        run_git(&work, &["add", "."])?;
        run_git(&work, &["commit", "-m", "feature"])?;
        run_git(&work, &["push", "-u", "origin", "HEAD"])?;

        // Switch back to base branch (whatever it is) and delete local feature branch
        let base_branch = git_out(&work, &["rev-parse", "--abbrev-ref", "@{-1}"])?;
        run_git(&work, &["checkout", &base_branch])?;
        run_git(&work, &["branch", "-D", "yk/compute_trie2"])?;
        // Ensure remote-tracking refs are present locally
        run_git(&work, &["fetch", "--all", "--prune"])?;

        // Point GitManager at the clone's root and validate
        let gm = GitManager { repo_root: work.to_string_lossy().into_owned() };
        gm.validate_refs(&["yk/compute_trie2"]) ?;
        Ok(())
    }

    #[test]
    fn validate_invalid_ref_fails() -> Result<()> {
        let (_tmp, _origin, work) = setup_repo()?;
        let gm = GitManager { repo_root: work.to_string_lossy().into_owned() };
        let res = gm.validate_refs(&["definitely-does-not-exist-branch-name-123"]);
        assert!(res.is_err());
        Ok(())
    }
}
