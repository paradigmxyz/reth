// Updates the benchmark PR comment with current status.
//
// Reads from environment:
//   BENCH_COMMENT_ID  – GitHub comment ID to update
//   BENCH_JOB_URL     – URL to the Actions job page
//   BENCH_CONFIG      – Config line (blocks, warmup, refs)
//   BENCH_ACTOR       – User who triggered the benchmark
//
// Usage from actions/github-script:
//   const s = require('./.github/scripts/bench-update-status.js');
//   await s({github, context, status: 'Building baseline binary...'});

function buildBody(status) {
  return `cc @${process.env.BENCH_ACTOR}\n\n🚀 Benchmark started! [View job](${process.env.BENCH_JOB_URL})\n\n⏳ **Status:** ${status}\n\n${process.env.BENCH_CONFIG}`;
}

async function updateStatus({ github, context, status }) {
  await github.rest.issues.updateComment({
    owner: context.repo.owner,
    repo: context.repo.repo,
    comment_id: parseInt(process.env.BENCH_COMMENT_ID),
    body: buildBody(status),
  });
}

updateStatus.buildBody = buildBody;
module.exports = updateStatus;
