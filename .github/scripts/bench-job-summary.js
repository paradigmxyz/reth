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
// Usage from actions/github-script:
//   const jobSummary = require('./.github/scripts/bench-job-summary.js');
//   await jobSummary({ core, context, chartSha, grafanaUrl, runId });

const fs = require('fs');
const { verdict, loadSamplyUrls, blocksLabel, metricRows, waitTimeRows } = require('./bench-utils');

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

  const { emoji, label } = verdict(summary.changes);
  const baselineLink = `[\`${summary.baseline.name}\`](${commitUrl}/${summary.baseline.ref})`;
  const featureLink = `[\`${summary.feature.name}\`](${commitUrl}/${summary.feature.ref})`;
  const diffUrl = `https://github.com/${repo}/compare/${summary.baseline.ref}...${summary.feature.ref}`;

  // Header & metadata
  const metaParts = [];
  if (prNumber) metaParts.push(`**[PR #${prNumber}](https://github.com/${repo}/pull/${prNumber})**`);
  metaParts.push(`triggered by @${actor}`);

  let md = `# ${emoji} ${label}\n\n`;
  md += metaParts.join(' · ') + '\n\n';
  md += `**Baseline:** ${baselineLink}\n`;
  md += `**Feature:** ${featureLink} ([diff](${diffUrl}))\n`;
  md += blocksLabel(summary).map(p => `**${p.key}:** ${p.value}`).join(' · ') + '\n\n';

  // Main comparison table
  const rows = metricRows(summary);
  md += `| Metric | Baseline | Feature | Change |\n`;
  md += `|--------|----------|---------|--------|\n`;
  for (const r of rows) {
    md += `| ${r.label} | ${r.baseline} | ${r.feature} | ${r.change} |\n`;
  }
  md += '\n';

  // Wait time breakdown
  const wtRows = waitTimeRows(summary);
  if (wtRows.length > 0) {
    md += `### Wait Time Breakdown\n\n`;
    md += `| Metric | Baseline | Feature |\n`;
    md += `|--------|----------|--------|\n`;
    for (const r of wtRows) {
      md += `| ${r.title} | ${r.baseline} | ${r.feature} |\n`;
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
  const samplyUrls = loadSamplyUrls(process.env.BENCH_WORK_DIR);
  const samplyLinks = Object.entries(samplyUrls).map(([run, url]) => `- **${run}**: [Firefox Profiler](${url})`);
  if (samplyLinks.length > 0) {
    md += `### Samply Profiles\n\n${samplyLinks.join('\n')}\n\n`;
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
