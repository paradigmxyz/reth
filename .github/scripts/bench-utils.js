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
