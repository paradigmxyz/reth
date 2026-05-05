// GitHub Actions helpers for resolving checkout/build refs.

const { execSync } = require('child_process');

function run(cmd) {
  return execSync(cmd, { encoding: 'utf8' }).trim();
}

async function resolveCheckoutRef({ github, context, core }) {
  if (!process.env.BENCH_PR) {
    core.setOutput('ref', context.ref);
    return;
  }
  const { data: pr } = await github.rest.pulls.get({
    owner: context.repo.owner,
    repo: context.repo.repo,
    pull_number: parseInt(process.env.BENCH_PR),
  });
  core.info(`PR #${process.env.BENCH_PR} (${pr.state}), using head SHA ${pr.head.sha}`);
  core.setOutput('ref', pr.head.sha);
}

async function resolvePrInfo({ github, context, core }) {
  if (process.env.BENCH_PR) {
    const { data: pr } = await github.rest.pulls.get({
      owner: context.repo.owner,
      repo: context.repo.repo,
      pull_number: parseInt(process.env.BENCH_PR),
    });
    core.setOutput('head-ref', pr.head.ref);
    core.setOutput('head-sha', pr.head.sha);
  } else {
    core.setOutput('head-ref', process.env.GITHUB_REF_NAME);
    core.setOutput('head-sha', process.env.GITHUB_SHA);
  }
}

async function resolveRefs({ core }) {
  const baselineArg = process.env.BASELINE_ARG || '';
  const featureArg = process.env.FEATURE_ARG || '';

  let baselineRef, baselineName, featureRef, featureName;

  if (baselineArg) {
    try { run(`git fetch origin "${baselineArg}" --quiet`); } catch {}
    try {
      baselineRef = run(`git rev-parse "${baselineArg}"`);
    } catch {
      baselineRef = run(`git rev-parse "origin/${baselineArg}"`);
    }
    baselineName = baselineArg;
  } else {
    try {
      baselineRef = run('git merge-base HEAD origin/main');
    } catch {
      baselineRef = process.env.GITHUB_SHA;
    }
    baselineName = 'main';
  }

  if (featureArg) {
    try { run(`git fetch origin "${featureArg}" --quiet`); } catch {}
    try {
      featureRef = run(`git rev-parse "${featureArg}"`);
    } catch {
      featureRef = run(`git rev-parse "origin/${featureArg}"`);
    }
    featureName = featureArg;
  } else {
    featureRef = process.env.PR_HEAD_SHA;
    featureName = process.env.PR_HEAD_REF;
  }

  core.setOutput('baseline-ref', baselineRef);
  core.setOutput('baseline-name', baselineName);
  core.setOutput('feature-ref', featureRef);
  core.setOutput('feature-name', featureName);
}

module.exports = {
  resolveCheckoutRef,
  resolvePrInfo,
  resolveRefs,
};
