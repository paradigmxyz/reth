// Generates a rich GitHub Actions job summary for benchmark results.
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

  const chartUrl = (file) => {
    if (!chartSha) return '';
    const prNum = prNumber || '0';
    return `https://raw.githubusercontent.com/decofe/reth-bench-charts/${chartSha}/pr/${prNum}/${runId}/${file}`;
  };

  // Main comparison table
  const rows = metricRows(summary);
  md += `| Headline metric | Baseline | Feature | Change |\n`;
  md += `|--------|----------|---------|--------|\n`;
  for (const r of rows) {
    md += `| ${r.label} | ${r.baseline} | ${r.feature} | ${r.change} |\n`;
  }
  md += '\n';
  if (chartSha) {
    md += `<details open><summary>Latency, throughput & diff</summary>\n\n`;
    md += `![Latency, throughput & diff](${chartUrl('latency_throughput.png')})\n\n`;
    md += `</details>\n\n`;
  }
  md += '> Noise: `±` is the 95% CI half-width. ⚪ means the change is within noise or ABBA cross-pairings disagree. Detailed rows show absolute deltas first so tiny baselines do not produce misleading percentages.\n\n';

  const prom = summary.prometheus || {};
  if (summary.bal_mode && prom.bal && prom.bal.length > 0) {
    md += `<details><summary>BAL metrics</summary>\n\n`;
    md += `| Metric | Baseline | Feature | Change |\n`;
    md += `|--------|----------|---------|--------|\n`;
    for (const r of prom.bal) md += `| ${r.label} | ${r.baseline_fmt} | ${r.feature_fmt} | ${r.change} |\n`;
    if (chartSha) md += `\n![BAL metrics](${chartUrl('bal.png')})\n`;
    md += `\n</details>\n\n`;
  }

  const phaseRows = (prom.phases || []).filter(r => summary.bal_mode || r.key !== 'bal_validation');
  if (phaseRows.length > 0) {
    md += `<details><summary>Execution phase breakdown</summary>\n\n`;
    md += `| Metric | Baseline | Feature | Change |\n`;
    md += `|--------|----------|---------|--------|\n`;
    for (const r of phaseRows) md += `| ${r.label} | ${r.baseline_fmt} | ${r.feature_fmt} | ${r.change} |\n`;
    if (chartSha) md += `\n![Prometheus execution phases](${chartUrl('prometheus_phases.png')})\n`;
    md += `\n</details>\n\n`;
  }

  if (prom.cache_trie && prom.cache_trie.length > 0) {
    md += `<details><summary>Cache and trie metrics</summary>\n\n`;
    md += `| Metric | Baseline | Feature | Change |\n`;
    md += `|--------|----------|---------|--------|\n`;
    for (const r of prom.cache_trie) md += `| ${r.label} | ${r.baseline_fmt} | ${r.feature_fmt} | ${r.change} |\n`;
    if (chartSha) md += `\n![Cache and trie](${chartUrl('cache_trie.png')})\n`;
    md += `\n</details>\n\n`;
  }

  // Wait time breakdown
  const wtRows = waitTimeRows(summary);
  if (wtRows.length > 0) {
    md += `### Wait Time Breakdown\n\n`;
    md += `| Metric | Baseline | Feature | Change |\n`;
    md += `|--------|----------|--------|--------|\n`;
    for (const r of wtRows) {
      md += `| ${r.title} | ${r.baseline} | ${r.feature} | ${r.change || ''} |\n`;
    }
    if (chartSha) md += `\n![Wait time breakdown](${chartUrl('wait_breakdown.png')})\n`;
    md += '\n';
  }

  if (chartSha) {
    md += `<details><summary>Gas vs latency</summary>\n\n`;
    md += `![Gas vs latency](${chartUrl('gas_vs_latency.png')})\n\n`;
    md += `</details>\n\n`;
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
