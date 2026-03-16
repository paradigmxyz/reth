// Generates a rich GitHub Actions job summary for reth-bench results.
//
// Reads from environment:
//   BENCH_WORK_DIR       – Directory containing summary.json
//   BENCH_PR             – PR number (may be empty)
//   BENCH_ACTOR          – GitHub user who triggered the bench
//   BENCH_CORES          – CPU core limit (0 = all)
//   BENCH_WARMUP_BLOCKS  – Number of warmup blocks
//   BENCH_SAMPLY         – 'true' if samply profiling was enabled
//   BENCH_ABBA           – 'true' if ABBA interleaved order was used
//
// Caller must pass chartSha, grafanaUrl via opts.
//
// Usage from actions/github-script:
//   const jobSummary = require('./.github/scripts/bench-job-summary.js');
//   await jobSummary({ core, context, chartSha, grafanaUrl, runId });

const fs = require('fs');
const path = require('path');

const SIG_EMOJI = { good: '✅', bad: '❌', neutral: '⚪' };

function fmtMs(v) { return v.toFixed(2) + 'ms'; }
function fmtMgas(v) { return v.toFixed(2); }
function fmtS(v) { return v.toFixed(2) + 's'; }

function fmtChange(ch) {
  if (!ch) return '';
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
  return { emoji: '⚪', label: 'No Significant Difference' };
}

module.exports = async function ({ core, context, chartSha, grafanaUrl, runId }) {
  let summary;
  try {
    summary = JSON.parse(fs.readFileSync(process.env.BENCH_WORK_DIR + '/summary.json', 'utf8'));
  } catch (e) {
    await core.summary.addRaw('⚠️ Benchmark completed but failed to load summary.').write();
    return;
  }

  const repo = `${context.repo.owner}/${context.repo.repo}`;
  const prNumber = process.env.BENCH_PR;
  const actor = process.env.BENCH_ACTOR;
  const commitUrl = `https://github.com/${repo}/commit`;
  const b = summary.baseline.stats;
  const f = summary.feature.stats;
  const c = summary.changes;

  const { emoji, label } = verdict(c);
  const baselineLink = `[\`${summary.baseline.name}\`](${commitUrl}/${summary.baseline.ref})`;
  const featureLink = `[\`${summary.feature.name}\`](${commitUrl}/${summary.feature.ref})`;
  const diffUrl = `https://github.com/${repo}/compare/${summary.baseline.ref}...${summary.feature.ref}`;

  // Header & metadata
  const metaParts = [];
  if (prNumber) metaParts.push(`**[PR #${prNumber}](https://github.com/${repo}/pull/${prNumber})**`);
  metaParts.push(`triggered by @${actor}`);
  const cores = process.env.BENCH_CORES || '0';

  let md = `# ${emoji} ${label}\n\n`;
  md += metaParts.join(' · ') + '\n\n';
  md += `**Baseline:** ${baselineLink}\n`;
  md += `**Feature:** ${featureLink} ([diff](${diffUrl}))\n`;

  const countsParts = [];
  if (summary.big_blocks) {
    if (summary.gas_ramp_blocks) countsParts.push(`**Gas Ramp:** ${summary.gas_ramp_blocks}`);
    countsParts.push(`**Big Blocks:** ${summary.blocks}`);
  } else {
    const warmup = summary.warmup_blocks || process.env.BENCH_WARMUP_BLOCKS || '';
    if (warmup) countsParts.push(`**Warmup:** ${warmup}`);
    countsParts.push(`**Blocks:** ${summary.blocks}`);
  }
  if (cores !== '0') countsParts.push(`**Cores:** ${cores}`);
  md += countsParts.join(' · ') + '\n\n';

  // Main comparison table
  md += `| Metric | Baseline | Feature | Change |\n`;
  md += `|--------|----------|---------|--------|\n`;
  md += `| Mean | ${fmtMs(b.mean_ms)} | ${fmtMs(f.mean_ms)} | ${fmtChange(c.mean)} |\n`;
  md += `| StdDev | ${fmtMs(b.stddev_ms)} | ${fmtMs(f.stddev_ms)} | |\n`;
  md += `| P50 | ${fmtMs(b.p50_ms)} | ${fmtMs(f.p50_ms)} | ${fmtChange(c.p50)} |\n`;
  md += `| P90 | ${fmtMs(b.p90_ms)} | ${fmtMs(f.p90_ms)} | ${fmtChange(c.p90)} |\n`;
  md += `| P99 | ${fmtMs(b.p99_ms)} | ${fmtMs(f.p99_ms)} | ${fmtChange(c.p99)} |\n`;
  md += `| Mgas/s | ${fmtMgas(b.mean_mgas_s)} | ${fmtMgas(f.mean_mgas_s)} | ${fmtChange(c.mgas_s)} |\n`;
  md += `| Wall Clock | ${fmtS(b.wall_clock_s)} | ${fmtS(f.wall_clock_s)} | ${fmtChange(c.wall_clock)} |\n\n`;

  // Wait time breakdown
  const waitTimes = summary.wait_times || {};
  const waitKeys = Object.keys(waitTimes);
  if (waitKeys.length > 0) {
    md += `### Wait Time Breakdown\n\n`;
    md += `| Metric | Baseline | Feature |\n`;
    md += `|--------|----------|--------|\n`;
    for (const key of waitKeys) {
      const wt = waitTimes[key];
      md += `| ${wt.title} (mean) | ${fmtMs(wt.baseline.mean_ms)} | ${fmtMs(wt.feature.mean_ms)} |\n`;
      md += `| ${wt.title} (p50) | ${fmtMs(wt.baseline.p50_ms)} | ${fmtMs(wt.feature.p50_ms)} |\n`;
      md += `| ${wt.title} (p95) | ${fmtMs(wt.baseline.p95_ms)} | ${fmtMs(wt.feature.p95_ms)} |\n`;
    }
    md += '\n';
  }

  // Charts
  if (chartSha) {
    const prNum = prNumber || '0';
    const baseUrl = `https://raw.githubusercontent.com/decofe/reth-bench-charts/${chartSha}/pr/${prNum}/${runId}`;
    const charts = [
      { file: 'latency_throughput.png', label: 'Latency, Throughput & Diff' },
      { file: 'wait_breakdown.png', label: 'Wait Time Breakdown' },
      { file: 'gas_vs_latency.png', label: 'Gas vs Latency' },
    ];
    md += `### Charts\n\n`;
    for (const chart of charts) {
      md += `<details><summary>${chart.label}</summary>\n\n`;
      md += `![${chart.label}](${baseUrl}/${chart.file})\n\n`;
      md += `</details>\n\n`;
    }
  }

  // Samply profiles
  if (process.env.BENCH_SAMPLY === 'true') {
    const abba = (process.env.BENCH_ABBA || 'true') !== 'false';
    const runs = abba ? ['baseline-1', 'feature-1', 'feature-2', 'baseline-2'] : ['baseline-1', 'feature-1'];
    const links = [];
    for (const run of runs) {
      try {
        const url = fs.readFileSync(path.join(process.env.BENCH_WORK_DIR, run, 'samply-profile-url.txt'), 'utf8').trim();
        if (url) links.push(`- **${run}**: [Firefox Profiler](${url})`);
      } catch {}
    }
    if (links.length > 0) {
      md += `### Samply Profiles\n\n${links.join('\n')}\n\n`;
    }
  }

  // Grafana
  if (grafanaUrl) {
    md += `### Grafana Dashboard\n\n[View real-time metrics](${grafanaUrl})\n\n`;
  }

  // Node errors
  try {
    const errors = fs.readFileSync(process.env.BENCH_WORK_DIR + '/errors.md', 'utf8');
    if (errors.trim()) md += '\n' + errors + '\n';
  } catch {}

  await core.summary.addRaw(md).write();
};
