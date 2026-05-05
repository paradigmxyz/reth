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
const {
  verdict,
  blocksLabel,
  metricRows,
  waitTimeRows,
  comparisonUrls,
  markdownChartSection,
  markdownSamplySection,
  markdownGrafanaSection,
  markdownErrorsSection,
} = require('./bench-utils');

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
  const urls = comparisonUrls(repo, summary, prNumber);

  const { emoji, label } = verdict(summary.changes);
  const baselineLink = `[\`${summary.baseline.name}\`](${urls.baseline})`;
  const featureLink = `[\`${summary.feature.name}\`](${urls.feature})`;

  // Header & metadata
  const metaParts = [];
  if (prNumber) metaParts.push(`**[PR #${prNumber}](https://github.com/${repo}/pull/${prNumber})**`);
  metaParts.push(`triggered by @${actor}`);

  let md = `# ${emoji} ${label}\n\n`;
  md += metaParts.join(' · ') + '\n\n';
  md += `**Baseline:** ${baselineLink}\n`;
  md += `**Feature:** ${featureLink} ([diff](${urls.diff}))\n`;
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

  md += markdownChartSection({ chartSha, prNumber, runId }).replace(/^\n\n/, '');
  md += markdownSamplySection(process.env.BENCH_WORK_DIR).replace(/^\n\n/, '');
  md += markdownGrafanaSection(grafanaUrl).replace(/^\n\n/, '');
  md += markdownErrorsSection(process.env.BENCH_WORK_DIR);

  await core.summary.addRaw(md).write();
};
