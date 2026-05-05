// Shared utilities for reth-bench result rendering.
//
// Used by bench-job-summary.js and bench-slack-notify.js.

const fs = require('fs');
const path = require('path');

const SIG_EMOJI = { good: '✅', bad: '❌', neutral: '⚪' };
const CHARTS = [
  { file: 'latency_throughput.png', label: 'Latency, Throughput & Diff' },
  { file: 'wait_breakdown.png', label: 'Wait Time Breakdown' },
  { file: 'gas_vs_latency.png', label: 'Gas vs Latency' },
];

function fmtMs(v) { return v.toFixed(2) + 'ms'; }
function fmtMgas(v) { return v.toFixed(2); }
function fmtS(v) { return v.toFixed(2) + 's'; }

function fmtChange(ch) {
  if (!ch || (!ch.pct && !ch.ci_pct)) return '';
  const pctStr = `${ch.pct >= 0 ? '+' : ''}${ch.pct.toFixed(2)}%`;
  const ciStr = ch.ci_pct ? ` (±${ch.ci_pct.toFixed(2)}%)` : '';
  return `${pctStr}${ciStr} ${SIG_EMOJI[ch.sig]}`;
}

function verdict(changes) {
  const vals = Object.values(changes);
  const hasBad = vals.some(v => v.sig === 'bad');
  const hasGood = vals.some(v => v.sig === 'good');
  if (hasBad && hasGood) return { emoji: '⚠️', label: 'Mixed Results' };
  if (hasBad) return { emoji: '❌', label: 'Regression' };
  if (hasGood) return { emoji: '✅', label: 'Improvement' };
  return { emoji: '⚪', label: 'No Difference' };
}

function runOrder(abba = (process.env.BENCH_ABBA || 'true') !== 'false') {
  return abba ? ['feature-1', 'baseline-1', 'baseline-2', 'feature-2'] : ['feature-1', 'baseline-1'];
}

function loadSamplyUrls(workDir) {
  const urls = {};
  for (const run of runOrder(true)) {
    try {
      const url = fs.readFileSync(path.join(workDir, run, 'samply-profile-url.txt'), 'utf8').trim();
      if (url) urls[run] = url;
    } catch {}
  }
  return urls;
}

function comparisonUrls(repo, summary, prNumber) {
  const commitUrl = `https://github.com/${repo}/commit`;
  return {
    pr: prNumber ? `https://github.com/${repo}/pull/${prNumber}` : '',
    baseline: `${commitUrl}/${summary.baseline.ref}`,
    feature: `${commitUrl}/${summary.feature.ref}`,
    diff: `https://github.com/${repo}/compare/${summary.baseline.ref}...${summary.feature.ref}`,
  };
}

function chartBaseUrl({ chartSha, prNumber, runId }) {
  return `https://raw.githubusercontent.com/decofe/reth-bench-charts/${chartSha}/pr/${prNumber || '0'}/${runId}`;
}

function markdownChartSection({ chartSha, prNumber, runId }) {
  if (!chartSha) return '';
  const baseUrl = chartBaseUrl({ chartSha, prNumber, runId });
  let md = '\n\n### Charts\n\n';
  for (const chart of CHARTS) {
    md += `<details><summary>${chart.label}</summary>\n\n`;
    md += `![${chart.label}](${baseUrl}/${chart.file})\n\n`;
    md += `</details>\n\n`;
  }
  return md;
}

function markdownSamplySection(workDir, abba = (process.env.BENCH_ABBA || 'true') !== 'false') {
  const urls = loadSamplyUrls(workDir);
  const links = runOrder(abba)
    .filter(run => urls[run])
    .map(run => `- **${run}**: [Firefox Profiler](${urls[run]})`);
  return links.length > 0 ? `\n\n### Samply Profiles\n\n${links.join('\n')}\n` : '';
}

function markdownGrafanaSection(grafanaUrl) {
  return grafanaUrl ? `\n\n### Grafana Dashboard\n\n[View real-time metrics](${grafanaUrl})\n` : '';
}

function markdownErrorsSection(workDir) {
  try {
    const errors = fs.readFileSync(path.join(workDir, 'errors.md'), 'utf8');
    if (errors.trim()) return '\n\n' + errors;
  } catch {}
  return '';
}

function balModeLabel(mode) {
  switch (mode) {
    case 'true':
    case 'feature':
    case 'baseline':
      return mode;
    case 'both':
      return 'true';
    default:
      return '';
  }
}

function blocksLabel(summary) {
  const parts = [];
  const driver = summary.driver || process.env.BENCH_DRIVER || '';
  if (driver) parts.push({ key: 'Driver', value: driver });
  if (summary.big_blocks) {
    parts.push({ key: 'Big Blocks', value: summary.blocks });
    const balMode = balModeLabel(summary.bal_mode || summary.bal || process.env.BENCH_BAL || 'false');
    if (balMode) parts.push({ key: 'BAL', value: balMode });
  } else {
    const warmup = summary.warmup_blocks || process.env.BENCH_WARMUP_BLOCKS || '';
    if (warmup) parts.push({ key: 'Warmup', value: warmup });
    parts.push({ key: 'Blocks', value: summary.blocks });
  }
  const cores = process.env.BENCH_CORES || '0';
  if (cores !== '0') parts.push({ key: 'Cores', value: cores });
  if (summary.wait_time) parts.push({ key: 'Wait time', value: summary.wait_time });
  return parts;
}

// The 7 metric rows shared by all renderers.
// Returns an array of { label, baseline, feature, change } objects.
function metricRows(summary) {
  const b = summary.baseline.stats;
  const f = summary.feature.stats;
  const c = summary.changes;
  return [
    { label: 'Mean',       baseline: fmtMs(b.mean_ms),       feature: fmtMs(f.mean_ms),       change: fmtChange(c.mean) },
    { label: 'StdDev',     baseline: fmtMs(b.stddev_ms),     feature: fmtMs(f.stddev_ms),     change: '' },
    { label: 'P50',        baseline: fmtMs(b.p50_ms),        feature: fmtMs(f.p50_ms),        change: fmtChange(c.p50) },
    { label: 'P90',        baseline: fmtMs(b.p90_ms),        feature: fmtMs(f.p90_ms),        change: fmtChange(c.p90) },
    { label: 'P99',        baseline: fmtMs(b.p99_ms),        feature: fmtMs(f.p99_ms),        change: fmtChange(c.p99) },
    { label: 'Mgas/s',     baseline: fmtMgas(b.mean_mgas_s), feature: fmtMgas(f.mean_mgas_s), change: fmtChange(c.mgas_s) },
    { label: 'Wall Clock', baseline: fmtS(b.wall_clock_s),   feature: fmtS(f.wall_clock_s),   change: fmtChange(c.wall_clock) },
    { label: 'Persist Wait', baseline: fmtMs(b.mean_persist_ms || 0), feature: fmtMs(f.mean_persist_ms || 0), change: fmtChange(c.persist_wait) },
  ];
}

// Wait time rows: one row per metric showing mean values.
function waitTimeRows(summary) {
  const waitTimes = summary.wait_times || {};
  const rows = [];
  for (const key of Object.keys(waitTimes)) {
    const wt = waitTimes[key];
    rows.push({ title: wt.title, baseline: fmtMs(wt.baseline.mean_ms), feature: fmtMs(wt.feature.mean_ms) });
  }
  return rows;
}

module.exports = {
  SIG_EMOJI,
  CHARTS,
  fmtMs,
  fmtMgas,
  fmtS,
  fmtChange,
  verdict,
  runOrder,
  loadSamplyUrls,
  comparisonUrls,
  chartBaseUrl,
  markdownChartSection,
  markdownSamplySection,
  markdownGrafanaSection,
  markdownErrorsSection,
  blocksLabel,
  metricRows,
  waitTimeRows,
};
