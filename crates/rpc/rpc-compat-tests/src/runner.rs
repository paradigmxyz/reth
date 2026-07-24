//! Embedded Reth execution and result classification.

use crate::{
    case::{discover, load_response_variant, RpcTestCase},
    config::ResolvedRunConfig,
    fixture::Fixture,
    matcher,
    schema::SchemaCatalog,
};
use alloy_rpc_types_engine::PayloadStatusEnum;
use eyre::{eyre, Context, Result};
use futures_util::future::BoxFuture;
use reth_e2e_test_utils::testsuite::{actions::Action, BlockInfo, Environment};
use reth_node_api::EngineTypes;
use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};
use tracing::{info, warn};

/// Action that executes the selected RPC compatibility tests.
#[derive(Debug)]
pub struct RunRpcCompatTests {
    fixture: Fixture,
    config: ResolvedRunConfig,
    schemas: SchemaCatalog,
}

impl RunRpcCompatTests {
    /// Creates a compatibility action.
    pub const fn new(fixture: Fixture, config: ResolvedRunConfig, schemas: SchemaCatalog) -> Self {
        Self { fixture, config, schemas }
    }
}

impl<Engine: EngineTypes> Action<Engine> for RunRpcCompatTests {
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let all = discover(&self.fixture.tests)?;
            self.config.validate_policy(all.iter().map(|test| test.id.as_str()))?;
            let tests = all.into_iter().filter(|test| self.config.selected(&test.id));
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(self.config.timeout_secs))
                .build()?;
            let url = env.node_clients[env.active_node_idx].rpc_url().clone();
            let started = Instant::now();
            let mut results = Vec::new();

            for test in tests {
                if self.config.skipped(&test.id) {
                    results.push(TestResult::new(&test, Outcome::Skip, 0, None));
                    continue;
                }
                let test_started = Instant::now();
                let result =
                    execute_test(&client, url.as_str(), &test, &self.config, &self.schemas).await;
                let elapsed = test_started.elapsed().as_millis();
                let passed = result.is_ok();
                let outcome = if self.config.ignored(&test.id) {
                    if passed {
                        Outcome::IgnoredPass
                    } else {
                        Outcome::IgnoredFail
                    }
                } else if self.config.expected_failure(&test.id) {
                    if passed {
                        Outcome::Xpass
                    } else {
                        Outcome::Xfail
                    }
                } else if passed {
                    Outcome::Pass
                } else {
                    Outcome::Fail
                };
                let detail = result.err().map(|error| format!("{error:#}"));
                info!(test = %test.id, ?outcome, "RPC compatibility result");
                results.push(TestResult::new(&test, outcome, elapsed, detail));
                if self.config.fail_fast && outcome.unexpected() {
                    break;
                }
            }

            let report = Report {
                fixture_revision: self.fixture.revision.clone(),
                choices: self.config.selections.clone(),
                duration_ms: started.elapsed().as_millis(),
                results,
            };
            report.print();
            if let Some(path) = &self.config.report_json {
                write_file(path, &serde_json::to_string_pretty(&report)?)?;
            }
            if let Some(path) = &self.config.report_junit {
                write_file(path, &report.junit())?;
            }
            if report.results.iter().any(|result| result.outcome.unexpected()) {
                Err(eyre!("RPC compatibility run contained unexpected results"))
            } else {
                Ok(())
            }
        })
    }
}

async fn execute_test(
    client: &reqwest::Client,
    url: &str,
    test: &RpcTestCase,
    config: &ResolvedRunConfig,
    schemas: &SchemaCatalog,
) -> Result<()> {
    let variants = config
        .responses
        .get(&test.id)
        .map(|paths| {
            paths.iter().map(|path| load_response_variant(path)).collect::<Result<Vec<_>>>()
        })
        .transpose()?;
    let mut failures = Vec::new();
    for (index, exchange) in test.exchanges.iter().enumerate() {
        if let Err(error) = execute_exchange(
            client,
            url,
            test,
            index,
            exchange,
            variants.as_deref(),
            config.ignore_error_data,
            schemas,
        )
        .await
        {
            failures.push(format!("{error:#}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(eyre!("{} exchange(s) failed:\n{}", failures.len(), failures.join("\n\n")))
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_exchange(
    client: &reqwest::Client,
    url: &str,
    test: &RpcTestCase,
    index: usize,
    exchange: &crate::case::RpcExchange,
    variants: Option<&[Vec<Value>]>,
    ignore_error_data: bool,
    schemas: &SchemaCatalog,
) -> Result<()> {
    let method = exchange.request["method"]
        .as_str()
        .ok_or_else(|| eyre!("exchange {} has no string method", index + 1))?;
    let body = client
        .post(url)
        .header("content-type", "application/json")
        .body(exchange.request_raw.clone())
        .send()
        .await
        .wrap_err_with(|| format!("{} exchange {} request failed", test.id, index + 1))?
        .text()
        .await?;
    let actual: Value = serde_json::from_str(body.trim()).wrap_err_with(|| {
        format!("{} exchange {} returned invalid JSON: {body}", test.id, index + 1)
    })?;
    let expected = if let Some(variants) = variants {
        variants.iter().filter_map(|variant| variant.get(index).cloned()).collect()
    } else {
        vec![exchange.expected.clone()]
    };
    match matcher::compare(&actual, &expected, test.spec_only, ignore_error_data, method, schemas) {
        Ok(()) => Ok(()),
        Err(error) => Err(eyre!(
            "{} exchange {}\n>>  {}\n<<  {}\n{error:#}",
            test.id,
            index + 1,
            exchange.request_raw,
            body
        )),
    }
}

/// Applies the fixture's initial forkchoice state.
#[derive(Debug, Clone)]
pub struct InitializeFixture {
    path: String,
}

impl InitializeFixture {
    /// Creates the initialization action.
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

impl<Engine: EngineTypes> Action<Engine> for InitializeFixture {
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let state =
                reth_e2e_test_utils::setup_import::load_forkchoice_state(Path::new(&self.path))?;
            for (index, client) in env.node_clients.iter().enumerate() {
                for attempt in 0..=10 {
                    let response =
                        reth_rpc_api::clients::EngineApiClient::<Engine>::fork_choice_updated_v3(
                            &client.engine.http_client(),
                            state,
                            None,
                        )
                        .await?;
                    match response.payload_status.status {
                        PayloadStatusEnum::Valid => break,
                        PayloadStatusEnum::Syncing if attempt < 10 => {
                            tokio::time::sleep(Duration::from_millis(500)).await;
                        }
                        status => {
                            return Err(eyre!(
                                "node {index} rejected fixture forkchoice: {status:?}"
                            ))
                        }
                    }
                }
            }
            env.active_node_state_mut()?.current_block_info =
                Some(BlockInfo { hash: state.head_block_hash, number: 0, timestamp: 0 });
            Ok(())
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Outcome {
    Pass,
    Fail,
    Xfail,
    Xpass,
    IgnoredPass,
    IgnoredFail,
    Skip,
}

impl Outcome {
    const fn unexpected(self) -> bool {
        matches!(self, Self::Fail | Self::Xpass)
    }
}

#[derive(Debug, Serialize)]
struct TestResult {
    id: String,
    description: String,
    outcome: Outcome,
    duration_ms: u128,
    detail: Option<String>,
}

impl TestResult {
    fn new(
        test: &RpcTestCase,
        outcome: Outcome,
        duration_ms: u128,
        detail: Option<String>,
    ) -> Self {
        Self {
            id: test.id.clone(),
            description: test.description.clone(),
            outcome,
            duration_ms,
            detail,
        }
    }
}

#[derive(Debug, Serialize)]
struct Report {
    fixture_revision: String,
    choices: std::collections::BTreeMap<String, String>,
    duration_ms: u128,
    results: Vec<TestResult>,
}

impl Report {
    fn print(&self) {
        for outcome in [
            Outcome::Pass,
            Outcome::Fail,
            Outcome::Xfail,
            Outcome::Xpass,
            Outcome::IgnoredPass,
            Outcome::IgnoredFail,
            Outcome::Skip,
        ] {
            let count = self
                .results
                .iter()
                .filter(|result| {
                    std::mem::discriminant(&result.outcome) == std::mem::discriminant(&outcome)
                })
                .count();
            if count > 0 {
                info!(?outcome, count, "RPC compatibility summary");
            }
        }
        for result in self.results.iter().filter(|result| result.outcome.unexpected()) {
            warn!(test = %result.id, detail = ?result.detail, "unexpected RPC compatibility result");
        }
        for result in self.results.iter().filter(|result| result.detail.is_some()) {
            eprintln!(
                "\n{:?} {}\n{}",
                result.outcome,
                result.id,
                result.detail.as_deref().unwrap_or_default()
            );
        }
    }

    fn junit(&self) -> String {
        let failures = self.results.iter().filter(|result| result.outcome.unexpected()).count();
        let skipped = self
            .results
            .iter()
            .filter(|result| {
                matches!(result.outcome, Outcome::Skip | Outcome::Xfail | Outcome::IgnoredFail)
            })
            .count();
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"rpc-compat\" tests=\"{}\" failures=\"{failures}\" skipped=\"{skipped}\">\n",
            self.results.len()
        );
        for result in &self.results {
            xml.push_str(&format!(
                "  <testcase name=\"{}\" time=\"{:.3}\">",
                escape(&result.id),
                result.duration_ms as f64 / 1000.0
            ));
            if result.outcome.unexpected() {
                xml.push_str(&format!(
                    "<failure message=\"{:?}\">{}</failure>",
                    result.outcome,
                    escape(result.detail.as_deref().unwrap_or(""))
                ));
            } else if matches!(
                result.outcome,
                Outcome::Skip | Outcome::Xfail | Outcome::IgnoredFail
            ) {
                xml.push_str(&format!("<skipped message=\"{:?}\"/>", result.outcome));
            }
            if !result.outcome.unexpected() &&
                let Some(detail) = &result.detail
            {
                xml.push_str(&format!("<system-out>{}</system-out>", escape(detail)));
            }
            xml.push_str("</testcase>\n");
        }
        xml.push_str("</testsuite>\n");
        xml
    }
}

fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents).wrap_err_with(|| format!("failed to write {}", path.display()))
}

fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
