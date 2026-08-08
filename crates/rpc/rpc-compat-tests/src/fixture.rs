//! Fixture revision resolution, verification, caching, and local overrides.

use crate::config::FixtureConfig;
use eyre::{eyre, Context, Result};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};
use tar::Archive;

/// Resolved execution-apis repository and test paths.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// Repository root, used for schema discovery.
    pub root: PathBuf,
    /// Directory containing `chain.rlp` and `.io` files.
    pub tests: PathBuf,
    /// Revision or local override description.
    pub revision: String,
}

/// Resolves a local fixture or downloads the configured revision.
pub async fn resolve(
    config: &FixtureConfig,
    base: &Path,
    local_override: Option<&Path>,
    revision_override: Option<&str>,
    offline: bool,
) -> Result<Fixture> {
    if let Some(path) = local_override.or(config.local_path.as_deref()) {
        return from_local(&absolute(base, path), &config.tests_dir)
    }

    validate_repository(&config.repository)?;
    let cache_root = absolute(base, &config.cache_dir);
    let pointer = cache_root.join(resolution_pointer_name(&config.repository, &config.branch));
    let revision = if let Some(revision) = revision_override {
        validate_revision(revision)?;
        revision.to_string()
    } else {
        validate_branch(&config.branch)?;
        if offline {
            fs::read_to_string(&pointer)
                .wrap_err_with(|| {
                    format!(
                        "no cached resolution for {}/{}; run `fetch` without --offline first",
                        config.repository, config.branch
                    )
                })?
                .trim()
                .to_string()
        } else {
            resolve_revision(config, &config.branch).await?
        }
    };
    let destination = cache_root.join(&revision);
    if let Ok(fixture) = from_local(&destination, &config.tests_dir) {
        if revision_override.is_none() && !offline {
            fs::create_dir_all(&cache_root)?;
            fs::write(&pointer, &revision)?;
        }
        return Ok(Fixture { revision, ..fixture })
    }
    if offline {
        return Err(eyre!(
            "fixture revision {revision} is not cached at {} and offline mode is enabled",
            destination.display()
        ))
    }

    fs::create_dir_all(&cache_root)
        .wrap_err_with(|| format!("failed to create fixture cache {}", cache_root.display()))?;
    let url = config.archive_url.clone().unwrap_or_else(|| {
        format!("https://github.com/{}/archive/{{revision}}.tar.gz", config.repository)
    });
    let url = url.replace("{revision}", &revision);
    tracing::info!(%url, %revision, "downloading RPC compatibility fixture");
    let response = reqwest::get(&url).await.wrap_err("fixture download failed")?;
    if !response.status().is_success() {
        return Err(eyre!("fixture download returned HTTP {}", response.status()))
    }
    let bytes = response.bytes().await.wrap_err("failed to read fixture archive")?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if let Some(expected) = &config.sha256 &&
        !expected.eq_ignore_ascii_case(&digest)
    {
        return Err(eyre!("fixture SHA-256 mismatch: expected {expected}, received {digest}"))
    }

    let temporary =
        tempfile::tempdir_in(&cache_root).wrap_err("failed to create fixture staging")?;
    Archive::new(GzDecoder::new(Cursor::new(bytes)))
        .unpack(temporary.path())
        .wrap_err("failed to extract fixture archive")?;
    let extracted = fs::read_dir(temporary.path())?
        .filter_map(|entry| entry.ok())
        .find(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .ok_or_else(|| eyre!("fixture archive did not contain a repository directory"))?
        .path();
    fs::rename(&extracted, &destination)
        .wrap_err_with(|| format!("failed to move fixture into cache {}", destination.display()))?;
    if revision_override.is_none() {
        fs::write(&pointer, &revision)?;
    }
    let fixture = from_local(&destination, &config.tests_dir)?;
    Ok(Fixture { revision, ..fixture })
}

async fn resolve_revision(config: &FixtureConfig, branch: &str) -> Result<String> {
    let (owner, repository) = repository_parts(&config.repository)?;
    let mut url =
        reqwest::Url::parse(&format!("https://api.github.com/repos/{owner}/{repository}/commits"))?;
    url.path_segments_mut().map_err(|_| eyre!("GitHub API URL cannot be a base"))?.push(branch);
    let mut request = reqwest::Client::new()
        .get(url)
        .header("user-agent", "reth-rpc-compat")
        .header("accept", "application/vnd.github+json");
    if let Some(token) = config
        .github_token_env
        .as_deref()
        .and_then(|name| std::env::var(name).ok())
        .filter(|token| !token.is_empty())
    {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.wrap_err("failed to resolve latest fixture revision")?;
    if !response.status().is_success() {
        return Err(eyre!("fixture revision lookup returned HTTP {}", response.status()))
    }
    let body = response.text().await.wrap_err("failed to read fixture revision lookup")?;
    let body: serde_json::Value =
        serde_json::from_str(&body).wrap_err("fixture revision lookup returned invalid JSON")?;
    body["sha"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| eyre!("fixture revision lookup did not return a commit SHA"))
}

fn validate_repository(repository: &str) -> Result<()> {
    repository_parts(repository).map(|_| ())
}

fn repository_parts(repository: &str) -> Result<(&str, &str)> {
    let Some((owner, name)) = repository.split_once('/') else {
        return Err(eyre!("fixture repository must use owner/name form: {repository:?}"))
    };
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Err(eyre!("fixture repository must use owner/name form: {repository:?}"))
    }
    Ok((owner, name))
}

fn validate_branch(branch: &str) -> Result<()> {
    if branch.is_empty() || branch.chars().any(char::is_control) {
        return Err(eyre!("fixture branch is invalid: {branch:?}"))
    }
    Ok(())
}

fn validate_revision(revision: &str) -> Result<()> {
    if revision.is_empty() ||
        !revision.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(eyre!("fixture revision contains unsupported characters: {revision:?}"))
    }
    Ok(())
}

fn resolution_pointer_name(repository: &str, branch: &str) -> String {
    let digest = Sha256::digest(format!("{repository}\0{branch}"));
    format!(".resolved-{digest:x}")
}

fn from_local(path: &Path, tests_dir: &Path) -> Result<Fixture> {
    let (root, tests) = if path.join("chain.rlp").is_file() {
        (path.parent().unwrap_or(path).to_path_buf(), path.to_path_buf())
    } else {
        (path.to_path_buf(), path.join(tests_dir))
    };
    for required in ["chain.rlp", "genesis.json", "headfcu.json"] {
        if !tests.join(required).is_file() {
            return Err(eyre!("fixture is missing {}", tests.join(required).display()))
        }
    }
    Ok(Fixture { root, tests, revision: format!("local:{}", path.display()) })
}

fn absolute(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::resolution_pointer_name;

    #[test]
    fn resolution_pointer_is_scoped_to_repository_and_branch() {
        let upstream = resolution_pointer_name("ethereum/execution-apis", "main");
        assert_eq!(upstream, resolution_pointer_name("ethereum/execution-apis", "main"));
        assert_ne!(upstream, resolution_pointer_name("example/execution-apis", "main"));
        assert_ne!(upstream, resolution_pointer_name("ethereum/execution-apis", "feature/rpc"));
    }
}
