// Shared utilities for reth-bench result rendering.
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

function loadSamplyUrls(workDir) {
  const urls = {};
  for (const run of ['baseline-1', 'baseline-2', 'feature-1', 'feature-2']) {
    try {
      const url = fs.readFileSync(path.join(workDir, run, 'samply-profile-url.txt'), 'utf8').trim();
      if (url) urls[run] = url;
    } catch {}
  }
  return urls;
}

function blocksLabel(summary) {
  const parts = [];
  if (summary.big_blocks) {
    parts.push({ key: 'Big Blocks', value: summary.blocks });
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
  loadSamplyUrls,
  blocksLabel,
  metricRows,
  waitTimeRows,
};
