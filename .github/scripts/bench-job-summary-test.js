const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');
const jobSummary = require('./bench-job-summary');

for (const mode of ['engine', 'call']) {
  test(`${mode} summary preserves attachments and trace downloads`, async (t) => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'bench-summary-'));
    t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
    const previous = { ...process.env };
    t.after(() => { process.env = previous; });
    process.env.BENCH_WORK_DIR = directory;
    process.env.BENCH_TRACING_CHROME = 'true';
    process.env.BENCH_RESULTS_URL = 'https://github.com/owner/repo/actions/runs/1/artifacts/2';
    const stats = { mean_ms: 1, stddev_ms: 0, p50_ms: 1, p90_ms: 1, p99_ms: 1, mean_mgas_s: 1 };
    fs.writeFileSync(path.join(directory, 'summary.json'), JSON.stringify({
      mode, changes: {}, blocks: 20,
      baseline: { name: 'main', ref: 'a'.repeat(40), stats },
      feature: { name: 'feature', ref: 'b'.repeat(40), stats },
    }));
    const charts = '\n### Charts\n\n![Latency](https://github.com/user-attachments/assets/chart)\n';
    if (mode === 'engine') fs.writeFileSync(path.join(directory, 'charts.md'), charts);
    let markdown;
    await jobSummary({
      core: { summary: { addRaw(value) { markdown = value; return { async write() {} }; } } },
      context: { repo: { owner: 'owner', repo: 'repo' } },
      logsUrl: 'https://example.com/logs',
      tracesUrl: 'https://example.com/traces',
      runId: '1',
    });
    assert.equal(markdown.includes(charts), mode === 'engine');
    assert.ok(markdown.includes(`[Download bench-results](${process.env.BENCH_RESULTS_URL})`));
    assert.ok(markdown.includes('[Logs](https://example.com/logs)'));
    assert.ok(markdown.includes('[Traces](https://example.com/traces)'));
    assert.ok(!markdown.includes('reth-bench-charts'));
  });
}
