// Shared utilities for benchmark result rendering.
//
// Used by bench-job-summary.js and bench-slack-notify.js.

const fs = require('fs');
const path = require('path');

const SIG_EMOJI = { good: '✅', bad: '❌', neutral: '⚪' };

function fmtMs(v) { return v.toFixed(2) + 'ms'; }
function fmtMgas(v) { return v.toFixed(2); }
function fmtS(v) { return v.toFixed(2) + 's'; }

function fmtChange(ch) {
  if (!ch || (!ch.pct && !ch.ci_pct)) return '';
  const pctStr = `${ch.pct >= 0 ? '+' : ''}${ch.pct.toFixed(2)}%`;
  const details = [];
  if (ch.ci_pct) details.push(`±${ch.ci_pct.toFixed(2)}%`);
  if (ch.floor_pct) details.push(`floor ${ch.floor_pct.toFixed(2)}%`);
  if (ch.materiality?.threshold_ms) {
    details.push(`materiality ${ch.materiality.threshold_ms.toFixed(2)}ms`);
  }
  if (ch.informational) details.push('informational');
  const detailStr = details.length ? ` (${details.join(', ')})` : '';
  const sig = ch.informational ? 'neutral' : ch.sig;
  return `${pctStr}${detailStr} ${SIG_EMOJI[sig]}`;
}

function verdict(changes) {
  const vals = Object.values(changes).filter(v => !v.informational);
  const hasBad = vals.some(v => v.sig === 'bad');
  const hasGood = vals.some(v => v.sig === 'good');
  if (hasBad && hasGood) return { emoji: '⚠️', label: 'Mixed Results' };
  if (hasBad) return { emoji: '❌', label: 'Regression' };
  if (hasGood) return { emoji: '✅', label: 'Improvement' };
  return { emoji: '⚪', label: 'No Difference' };
}

function isWin(changes) {
  const vals = Object.values(changes || {}).filter(v => !v.informational);
  return vals.some(v => v.sig === 'good') && !vals.some(v => v.sig === 'bad');
}

function loadSamplyUrls(workDir) {
  return loadProfileUrls(workDir, 'samply-profile-url.txt');
}

function loadTracingChromeUrls(workDir) {
  return loadProfileUrls(workDir, 'tracing-chrome-profile-url.txt');
}

function loadProfileUrls(workDir, fileName) {
  const urls = {};
  let runs = [];
  try {
    runs = fs.readdirSync(workDir)
      .filter(run => /^(baseline|feature)-\d+$/.test(run))
      .sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
  } catch {
    return urls;
  }
  for (const run of runs) {
    try {
      const url = fs.readFileSync(path.join(workDir, run, fileName), 'utf8').trim();
      if (url) urls[run] = url;
    } catch {}
  }
  return urls;
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
  if (summary.mode === 'call') {
    const corpus = summary.corpus || {};
    parts.push({ key: 'Corpus', value: corpus.name || 'static' });
    if (corpus.class) parts.push({ key: 'Class', value: corpus.class });
    if (corpus.records) parts.push({ key: 'Records', value: corpus.records });
    if (summary.rps) parts.push({ key: 'Rps', value: summary.rps });
    if (summary.passes) parts.push({ key: 'Passes', value: summary.passes });
    const callRunPairs = summary.run_pairs || process.env.BENCH_RUN_PAIRS || '';
    if (callRunPairs) parts.push({ key: 'Run pairs', value: callRunPairs });
    return parts;
  }
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
  const runPairs = summary.run_pairs || process.env.BENCH_RUN_PAIRS || '';
  if (runPairs) {
    parts.push({ key: 'Run pairs', value: runPairs });
  }
  return parts;
}

// The metric rows shared by all renderers.
// Returns an array of { label, baseline, feature, change } objects.
function metricRows(summary) {
  const b = summary.baseline.stats;
  const f = summary.feature.stats;
  const c = summary.changes;
  if (summary.mode === 'call') {
    const optMs = v => (Number.isFinite(v) ? fmtMs(v) : 'n/a');
    const optNum = v => (Number.isFinite(v) ? v.toFixed(2) : 'n/a');
    const optPct = v => (Number.isFinite(v) ? `${v.toFixed(2)}%` : 'n/a');
    return [
      { label: 'Mean',            baseline: optMs(b.mean_ms),  feature: optMs(f.mean_ms),  change: fmtChange(c.mean) },
      { label: 'P50',             baseline: optMs(b.p50_ms),   feature: optMs(f.p50_ms),   change: fmtChange(c.p50) },
      { label: 'P90',             baseline: optMs(b.p90_ms),   feature: optMs(f.p90_ms),   change: fmtChange(c.p90) },
      { label: 'P99',             baseline: optMs(b.p99_ms),   feature: optMs(f.p99_ms),   change: fmtChange(c.p99) },
      { label: 'Record median',   baseline: optMs(b.record_median_ms), feature: optMs(f.record_median_ms), change: fmtChange(c.record_median) },
      { label: 'Closed-loop rps', baseline: optNum(b.closed_loop_rps), feature: optNum(f.closed_loop_rps), change: fmtChange(c.closed_loop_rps) },
      { label: 'CPU / request',   baseline: optMs(b.cpu_ms_per_request), feature: optMs(f.cpu_ms_per_request), change: fmtChange(c.cpu_per_request) },
      { label: 'Error rate',      baseline: optPct(b.error_rate_pct), feature: optPct(f.error_rate_pct), change: '' },
    ];
  }
  const optS = v => (Number.isFinite(v) ? fmtS(v) : 'n/a');
  const optMgas = v => (Number.isFinite(v) ? fmtMgas(v) : 'n/a');
  return [
    { label: 'Execution Mean',   baseline: fmtMs(b.mean_ms),       feature: fmtMs(f.mean_ms),       change: fmtChange(c.mean) },
    { label: 'Execution StdDev', baseline: fmtMs(b.stddev_ms),     feature: fmtMs(f.stddev_ms),     change: '' },
    { label: 'Execution P50',    baseline: fmtMs(b.p50_ms),        feature: fmtMs(f.p50_ms),        change: fmtChange(c.p50) },
    { label: 'Execution P90',    baseline: fmtMs(b.p90_ms),        feature: fmtMs(f.p90_ms),        change: fmtChange(c.p90) },
    { label: 'Execution P99',    baseline: fmtMs(b.p99_ms),        feature: fmtMs(f.p99_ms),        change: fmtChange(c.p99) },
    { label: 'Execution Mgas/s', baseline: fmtMgas(b.mean_mgas_s), feature: fmtMgas(f.mean_mgas_s), change: fmtChange(c.mgas_s) },
    { label: 'Wall Clock', baseline: optS(b.wall_clock_s), feature: optS(f.wall_clock_s), change: fmtChange(c.wall_clock) },
    { label: 'End-to-end Mgas/s', baseline: optMgas(b.end_to_end_mgas_s), feature: optMgas(f.end_to_end_mgas_s), change: fmtChange(c.end_to_end_mgas_s) },
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
  fmtMs,
  fmtMgas,
  fmtS,
  fmtChange,
  verdict,
  isWin,
  loadSamplyUrls,
  loadTracingChromeUrls,
  blocksLabel,
  metricRows,
  waitTimeRows,
};
