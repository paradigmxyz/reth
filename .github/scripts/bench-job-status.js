// Resolves the Actions job URL and updates the benchmark status comment.

const { configLine } = require('./bench-request.js');
const { buildBody } = require('./bench-update-status.js');

async function resolveJobUrlAndUpdateStatus({ github, context, core, baselineName, featureName }) {
  const { data: jobs } = await github.rest.actions.listJobsForWorkflowRun({
    owner: context.repo.owner,
    repo: context.repo.repo,
    run_id: context.runId,
  });
  const expectedJobName = `bench-${process.env.BENCH_DRIVER || 'txgen'}`;
  const job = jobs.jobs.find(j => j.name === expectedJobName);
  const jobUrl = job ? job.html_url : `${context.serverUrl}/${context.repo.owner}/${context.repo.repo}/actions/runs/${context.runId}`;
  core.exportVariable('BENCH_JOB_URL', jobUrl);

  const bigBlocks = process.env.BENCH_BIG_BLOCKS === 'true';
  const config = configLine({
    blocks: process.env.BENCH_BLOCKS,
    warmup: process.env.BENCH_WARMUP_BLOCKS,
    baseline: baselineName,
    feature: featureName,
    samply: process.env.BENCH_SAMPLY === 'true',
    slack: process.env.BENCH_SLACK || 'always',
    bigBlocks,
    bal: process.env.BENCH_BAL || 'false',
    cores: process.env.BENCH_CORES || '0',
    abba: (process.env.BENCH_ABBA || 'true') !== 'false',
    otlp: (process.env.BENCH_OTLP || 'true') !== 'false',
    waitTime: process.env.BENCH_WAIT_TIME || '',
    baselineArgs: process.env.BENCH_BASELINE_ARGS || '',
    featureArgs: process.env.BENCH_FEATURE_ARGS || '',
    driver: process.env.BENCH_DRIVER || (bigBlocks ? 'reth-bench' : 'txgen'),
  });
  core.exportVariable('BENCH_CONFIG', config);

  await github.rest.issues.updateComment({
    owner: context.repo.owner,
    repo: context.repo.repo,
    comment_id: parseInt(process.env.BENCH_COMMENT_ID),
    body: buildBody('Building binaries...'),
  });
}

module.exports = { resolveJobUrlAndUpdateStatus };
