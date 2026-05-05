// Updates benchmark PR comments for success, failure, and cancellation.

const fs = require('fs');
const {
  markdownChartSection,
  markdownSamplySection,
  markdownGrafanaSection,
  markdownErrorsSection,
} = require('./bench-utils');

function defaultJobUrl(context) {
  return process.env.BENCH_JOB_URL || `${context.serverUrl}/${context.repo.owner}/${context.repo.repo}/actions/runs/${context.runId}`;
}

async function compareAndComment({ github, context, chartSha, grafanaUrl, runId }) {
  let comment = '';
  try {
    comment = fs.readFileSync(process.env.BENCH_WORK_DIR + '/comment.md', 'utf8');
  } catch (e) {
    comment = '⚠️ Engine benchmark completed but failed to generate comparison.';
  }

  const prNumber = process.env.BENCH_PR || '0';
  comment += markdownChartSection({ chartSha, prNumber, runId });
  comment += markdownSamplySection(process.env.BENCH_WORK_DIR);
  comment += markdownGrafanaSection(grafanaUrl);
  comment += markdownErrorsSection(process.env.BENCH_WORK_DIR);

  const body = `cc @${process.env.BENCH_ACTOR}\n\n✅ Benchmark complete! [View job](${defaultJobUrl(context)})\n\n${comment}`;
  await github.rest.issues.updateComment({
    owner: context.repo.owner,
    repo: context.repo.repo,
    comment_id: parseInt(process.env.BENCH_COMMENT_ID),
    body,
  });
}

async function updateFailed({ github, context, failedStep }) {
  const errorDetails = markdownErrorsSection(process.env.BENCH_WORK_DIR);

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
