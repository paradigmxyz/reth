//! Runner configuration and command-line override resolution.

use eyre::{eyre, Context, Result};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

/// Complete RPC compatibility runner configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Upstream fixture acquisition settings.
    pub fixture: FixtureConfig,
    /// Default execution settings.
    pub run: RunConfig,
    /// Optional named groups of execution overrides.
    pub profiles: BTreeMap<String, Profile>,
    /// Optional named choices with mutually exclusive options.
    pub choices: BTreeMap<String, Choice>,
}

impl Config {
    /// Loads configuration and returns it with the directory used to resolve relative paths.
    pub fn load(path: &Path) -> Result<(Self, PathBuf)> {
        let contents = fs::read_to_string(path)
            .wrap_err_with(|| format!("failed to read configuration {}", path.display()))?;
        let config = toml::from_str(&contents)
            .wrap_err_with(|| format!("failed to parse configuration {}", path.display()))?;
        let base = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        Ok((config, base))
    }

    /// Resolves profiles, choices, and command-line additions into one execution policy.
    pub fn resolve_run(
        &self,
        base: &Path,
        selected_profiles: &[String],
        selected_choices: &BTreeMap<String, String>,
        additions: &RunAdditions,
    ) -> Result<ResolvedRunConfig> {
        let mut resolved = ResolvedRunConfig::from_run(&self.run, base);
        let mut profiles = self.run.profiles.clone();
        profiles.extend(selected_profiles.iter().cloned());

        for name in profiles {
            let profile = self
                .profiles
                .get(&name)
                .ok_or_else(|| eyre!("unknown RPC compatibility profile {name:?}"))?;
            resolved.apply_profile(profile, base);
        }

        for (name, choice) in &self.choices {
            let option_name = selected_choices
                .get(name)
                .or(choice.default.as_ref())
                .ok_or_else(|| eyre!("choice {name:?} has no selection and no default"))?;
            let option = choice.options.get(option_name).ok_or_else(|| {
                eyre!("unknown option {option_name:?} for RPC compatibility choice {name:?}")
            })?;
            resolved.apply_profile(option, base);
            resolved.selections.insert(name.clone(), option_name.clone());
        }

        for name in selected_choices.keys() {
            if !self.choices.contains_key(name) {
                return Err(eyre!("unknown RPC compatibility choice {name:?}"))
            }
        }

        if !additions.include.is_empty() {
            resolved.include.clone_from(&additions.include);
        }
        resolved.exclude.extend(additions.exclude.iter().cloned());
        resolved.skip.extend(additions.skip.iter().cloned());
        resolved.ignore.extend(additions.ignore.iter().cloned());
        resolved.expected_failures.extend(additions.expected_failures.iter().cloned());
        resolved
            .expected_failures_when_error_data_checked
            .extend(additions.expected_failures_when_error_data_checked.iter().cloned());
        if additions.fail_fast {
            resolved.fail_fast = true;
        }
        if let Some(timeout_secs) = additions.timeout_secs {
            resolved.timeout_secs = timeout_secs;
        }
        if let Some(path) = &additions.report_json {
            resolved.report_json = Some(path.clone());
        }
        if let Some(path) = &additions.report_junit {
            resolved.report_junit = Some(path.clone());
        }
        Ok(resolved)
    }
}

/// Upstream fixture configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FixtureConfig {
    /// `GitHub` repository in `owner/name` form.
    pub repository: String,
    /// Branch to resolve to its latest commit on every online fetch.
    pub branch: String,
    /// Optional environment variable containing a `GitHub` API token.
    pub github_token_env: Option<String>,
    /// Optional SHA-256 of the downloaded archive.
    pub sha256: Option<String>,
    /// Optional archive URL template. `{revision}` is replaced before download.
    pub archive_url: Option<String>,
    /// Directory where downloaded revisions are cached.
    pub cache_dir: PathBuf,
    /// Fixture directory within the repository.
    pub tests_dir: PathBuf,
    /// Optional local repository or fixture directory override.
    pub local_path: Option<PathBuf>,
}

impl Default for FixtureConfig {
    fn default() -> Self {
        Self {
            repository: "ethereum/execution-apis".to_string(),
            branch: "main".to_string(),
            github_token_env: Some("GITHUB_TOKEN".to_string()),
            sha256: None,
            archive_url: None,
            cache_dir: PathBuf::from(".cache"),
            tests_dir: PathBuf::from("tests"),
            local_path: None,
        }
    }
}

/// Default execution behavior.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunConfig {
    /// Inclusion patterns. Empty means all tests.
    pub include: Vec<String>,
    /// Exclusion patterns.
    pub exclude: Vec<String>,
    /// Tests that are discovered but not executed.
    pub skip: Vec<String>,
    /// Tests whose outcome never affects the exit status.
    pub ignore: Vec<String>,
    /// Tests expected to fail. Unexpected passes fail the run.
    pub expected_failures: Vec<String>,
    /// Tests expected to fail only when JSON-RPC error `data` is compared.
    pub expected_failures_when_error_data_checked: Vec<String>,
    /// Profiles enabled by default.
    pub profiles: Vec<String>,
    /// Per-request timeout.
    pub timeout_secs: u64,
    /// Stops after the first unexpected result.
    pub fail_fast: bool,
    /// Ignore the `data` field in matching JSON-RPC error objects.
    pub ignore_error_data: bool,
    /// Optional JSON report path.
    pub report_json: Option<PathBuf>,
    /// Optional `JUnit` XML report path.
    pub report_junit: Option<PathBuf>,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            include: vec!["*".to_string()],
            exclude: Vec::new(),
            skip: Vec::new(),
            ignore: Vec::new(),
            expected_failures: Vec::new(),
            expected_failures_when_error_data_checked: Vec::new(),
            profiles: Vec::new(),
            timeout_secs: 5,
            fail_fast: false,
            ignore_error_data: false,
            report_json: None,
            report_junit: None,
        }
    }
}

/// Named execution override.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    /// Inclusion patterns. A non-empty list replaces the previous selection.
    pub include: Vec<String>,
    /// Additional exclusion patterns.
    pub exclude: Vec<String>,
    /// Additional skipped tests.
    pub skip: Vec<String>,
    /// Additional ignored tests.
    pub ignore: Vec<String>,
    /// Additional expected failures.
    pub expected_failures: Vec<String>,
    /// Additional failures expected only when JSON-RPC error `data` is compared.
    pub expected_failures_when_error_data_checked: Vec<String>,
    /// Alternative `.io` or JSON response files keyed by canonical test ID.
    pub responses: BTreeMap<String, Vec<PathBuf>>,
}

/// One named choice with mutually exclusive profile-like options.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Choice {
    /// Option selected when the user does not override the choice.
    pub default: Option<String>,
    /// Available options.
    pub options: BTreeMap<String, Profile>,
}

/// Command-line additions to the configured execution policy.
#[derive(Debug, Clone, Default)]
pub struct RunAdditions {
    /// Additional inclusion patterns.
    pub include: Vec<String>,
    /// Additional exclusion patterns.
    pub exclude: Vec<String>,
    /// Additional skipped tests.
    pub skip: Vec<String>,
    /// Additional ignored tests.
    pub ignore: Vec<String>,
    /// Additional expected failures.
    pub expected_failures: Vec<String>,
    /// Additional failures expected only when JSON-RPC error `data` is compared.
    pub expected_failures_when_error_data_checked: Vec<String>,
    /// Enable fail-fast.
    pub fail_fast: bool,
    /// Override timeout.
    pub timeout_secs: Option<u64>,
    /// Override JSON report path.
    pub report_json: Option<PathBuf>,
    /// Override `JUnit` report path.
    pub report_junit: Option<PathBuf>,
}

/// Fully resolved execution configuration.
#[derive(Debug, Clone)]
pub struct ResolvedRunConfig {
    /// Inclusion patterns.
    pub include: Vec<String>,
    /// Exclusion patterns.
    pub exclude: Vec<String>,
    /// Skip patterns.
    pub skip: Vec<String>,
    /// Ignore patterns.
    pub ignore: Vec<String>,
    /// Expected-failure patterns.
    pub expected_failures: Vec<String>,
    /// Patterns expected to fail only when JSON-RPC error `data` is compared.
    pub expected_failures_when_error_data_checked: Vec<String>,
    /// Alternative response files keyed by canonical test ID.
    pub responses: BTreeMap<String, Vec<PathBuf>>,
    /// Applied named choices.
    pub selections: BTreeMap<String, String>,
    /// Per-request timeout.
    pub timeout_secs: u64,
    /// Whether to stop on the first unexpected result.
    pub fail_fast: bool,
    /// Whether to ignore the `data` field in matching JSON-RPC error objects.
    pub ignore_error_data: bool,
    /// JSON report path.
    pub report_json: Option<PathBuf>,
    /// `JUnit` report path.
    pub report_junit: Option<PathBuf>,
}

impl ResolvedRunConfig {
    fn from_run(run: &RunConfig, base: &Path) -> Self {
        Self {
            include: run.include.clone(),
            exclude: run.exclude.clone(),
            skip: run.skip.clone(),
            ignore: run.ignore.clone(),
            expected_failures: run.expected_failures.clone(),
            expected_failures_when_error_data_checked: run
                .expected_failures_when_error_data_checked
                .clone(),
            responses: BTreeMap::new(),
            selections: BTreeMap::new(),
            timeout_secs: run.timeout_secs,
            fail_fast: run.fail_fast,
            ignore_error_data: run.ignore_error_data,
            report_json: run.report_json.as_ref().map(|path| base.join(path)),
            report_junit: run.report_junit.as_ref().map(|path| base.join(path)),
        }
    }

    fn apply_profile(&mut self, profile: &Profile, base: &Path) {
        if !profile.include.is_empty() {
            self.include.clone_from(&profile.include);
        }
        self.exclude.extend(profile.exclude.iter().cloned());
        self.skip.extend(profile.skip.iter().cloned());
        self.ignore.extend(profile.ignore.iter().cloned());
        self.expected_failures.extend(profile.expected_failures.iter().cloned());
        self.expected_failures_when_error_data_checked
            .extend(profile.expected_failures_when_error_data_checked.iter().cloned());
        for (test, paths) in &profile.responses {
            self.responses
                .entry(test.clone())
                .or_default()
                .extend(paths.iter().map(|path| base.join(path)));
        }
    }

    /// Returns true when a test is selected by the include/exclude policy.
    pub fn selected(&self, id: &str) -> bool {
        let included = self.include.is_empty() || matches_any(&self.include, id);
        included && !matches_any(&self.exclude, id)
    }

    /// Returns true when a test is skipped.
    pub fn skipped(&self, id: &str) -> bool {
        matches_any(&self.skip, id)
    }

    /// Returns true when a test is ignored.
    pub fn ignored(&self, id: &str) -> bool {
        matches_any(&self.ignore, id)
    }

    /// Returns true when a test is expected to fail.
    pub fn expected_failure(&self, id: &str) -> bool {
        matches_any(&self.expected_failures, id) ||
            !self.ignore_error_data &&
                matches_any(&self.expected_failures_when_error_data_checked, id)
    }

    /// Validates that exact policy entries refer to discovered tests.
    pub fn validate_policy<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> Result<()> {
        let ids = ids.into_iter().collect::<BTreeSet<_>>();
        for (kind, patterns) in [
            ("skip", &self.skip),
            ("ignore", &self.ignore),
            ("expected failure", &self.expected_failures),
            (
                "error-data-dependent expected failure",
                &self.expected_failures_when_error_data_checked,
            ),
        ] {
            for pattern in patterns {
                if !contains_wildcard(pattern) && !ids.contains(pattern.as_str()) {
                    return Err(eyre!("{kind} entry {pattern:?} does not match a discovered test"))
                }
            }
        }
        Ok(())
    }
}

/// Returns true if any wildcard pattern matches `text`.
pub fn matches_any(patterns: &[String], text: &str) -> bool {
    patterns.iter().any(|pattern| wildcard_match(pattern, text))
}

fn contains_wildcard(pattern: &str) -> bool {
    pattern.bytes().any(|byte| matches!(byte, b'*' | b'?'))
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let mut row = vec![false; text.len() + 1];
    row[0] = true;
    for &token in pattern {
        let previous = row.clone();
        row[0] = token == b'*' && previous[0];
        for index in 1..=text.len() {
            row[index] = match token {
                b'*' => previous[index] || row[index - 1],
                b'?' => previous[index - 1],
                literal => previous[index - 1] && literal == text[index - 1],
            };
        }
    }
    row[text.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_patterns_match_canonical_ids() {
        assert!(wildcard_match("eth_getBlockBy*", "eth_getBlockByHash/get-latest"));
        assert!(wildcard_match("*/get-invalid-?", "eth_call/get-invalid-1"));
        assert!(!wildcard_match("debug_*", "eth_call/simple"));
    }

    #[test]
    fn resolves_error_data_dependent_expected_failures() {
        let strict: Config = toml::from_str(
            "[run]\nexpected_failures_when_error_data_checked = [\"eth_test/case\"]",
        )
        .unwrap();
        let strict = strict
            .resolve_run(Path::new("."), &[], &BTreeMap::new(), &RunAdditions::default())
            .unwrap();
        assert!(strict.expected_failure("eth_test/case"));

        let ignored: Config = toml::from_str(
            "[run]\nignore_error_data = true\nexpected_failures_when_error_data_checked = [\"eth_test/case\"]",
        )
        .unwrap();
        let ignored = ignored
            .resolve_run(Path::new("."), &[], &BTreeMap::new(), &RunAdditions::default())
            .unwrap();
        assert!(!ignored.expected_failure("eth_test/case"));
    }
}
