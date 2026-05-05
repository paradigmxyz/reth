// Updates benchmark PR comments for success, failure, and cancellation.

const fs = require('fs');

function defaultJobUrl(context) {
  return process.env.BENCH_JOB_URL || `${context.serverUrl}/${context.repo.owner}/${context.repo.repo}/actions/runs/${context.runId}`;
}

function appendCharts(comment, { sha, prNumber, runId }) {
  if (!sha) return comment;
  const baseUrl = `https://raw.githubusercontent.com/decofe/reth-bench-charts/${sha}/pr/${prNumber}/${runId}`;
  const charts = [
    { file: 'latency_throughput.png', label: 'Latency, Throughput & Diff' },
    { file: 'wait_breakdown.png', label: 'Wait Time Breakdown' },
    { file: 'gas_vs_latency.png', label: 'Gas vs Latency' },
  ];
  let chartMarkdown = '\n\n### Charts\n\n';
  for (const chart of charts) {
    chartMarkdown += `<details><summary>${chart.label}</summary>\n\n`;
    chartMarkdown += `![${chart.label}](${baseUrl}/${chart.file})\n\n`;
    chartMarkdown += `</details>\n\n`;
  }
  return comment + chartMarkdown;
}

function appendSamplyProfiles(comment) {
  if (process.env.BENCH_SAMPLY !== 'true') return comment;
  const abba = (process.env.BENCH_ABBA || 'true') !== 'false';
  const runs = abba ? ['feature-1', 'baseline-1', 'baseline-2', 'feature-2'] : ['feature-1', 'baseline-1'];
  const links = [];
  for (const run of runs) {
    try {
      const url = fs.readFileSync(`${process.env.BENCH_WORK_DIR}/${run}/samply-profile-url.txt`, 'utf8').trim();
      if (url) {
        links.push(`- **${run}**: [Firefox Profiler](${url})`);
      }
    } catch (e) {}
  }
  if (links.length > 0) {
    return comment + `\n\n### Samply Profiles\n\n${links.join('\n')}\n`;
  }
  return comment;
}

function appendGrafana(comment, grafanaUrl) {
  if (!grafanaUrl) return comment;
  return comment + `\n\n### Grafana Dashboard\n\n[View real-time metrics](${grafanaUrl})\n`;
}

function appendErrors(comment) {
  try {
    const errors = fs.readFileSync(process.env.BENCH_WORK_DIR + '/errors.md', 'utf8');
    if (errors.trim()) {
      return comment + '\n\n' + errors;
    }
  } catch (e) {}
  return comment;
}

async function compareAndComment({ github, context, chartSha, grafanaUrl, runId }) {
  let comment = '';
  try {
    comment = fs.readFileSync(process.env.BENCH_WORK_DIR + '/comment.md', 'utf8');
  } catch (e) {
    comment = '⚠️ Engine benchmark completed but failed to generate comparison.';
  }

  const prNumber = process.env.BENCH_PR || '0';
  comment = appendCharts(comment, { sha: chartSha, prNumber, runId });
  comment = appendSamplyProfiles(comment);
  comment = appendGrafana(comment, grafanaUrl);
  comment = appendErrors(comment);

  const body = `cc @${process.env.BENCH_ACTOR}\n\n✅ Benchmark complete! [View job](${defaultJobUrl(context)})\n\n${comment}`;
  await github.rest.issues.updateComment({
    owner: context.repo.owner,
    repo: context.repo.repo,
    comment_id: parseInt(process.env.BENCH_COMMENT_ID),
    body,
  });
}

async function updateFailed({ github, context, failedStep }) {
  let errorDetails = '';
  try {
    const errors = fs.readFileSync(process.env.BENCH_WORK_DIR + '/errors.md', 'utf8');
    if (errors.trim()) {
      errorDetails = '\n\n' + errors;
    }
  } catch (e) {}

  await github.rest.issues.updateComment({
    owner: context.repo.owner,
    repo: context.repo.repo,
    comment_id: parseInt(process.env.BENCH_COMMENT_ID),
    body: `cc @${process.env.BENCH_ACTOR}\n\n❌ Benchmark failed while ${failedStep}. [View logs](${defaultJobUrl(context)})${errorDetails}`,
  });
}

async function updateCancelled({ github, context }) {
  await github.rest.issues.updateComment({
    owner: context.repo.owner,
    repo: context.repo.repo,
    comment_id: parseInt(process.env.BENCH_COMMENT_ID),
    body: `cc @${process.env.BENCH_ACTOR}\n\n⚠️ Benchmark cancelled. [View logs](${defaultJobUrl(context)})`,
  });
}

module.exports = {
  compareAndComment,
  updateFailed,
  updateCancelled,
};
