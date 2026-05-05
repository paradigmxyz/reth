// Handles benchmark request parsing, acknowledgement, and queue updates.

const validBalModes = new Set(['false', 'true', 'feature', 'baseline']);
const validSlackModes = new Set(['always', 'on-win', 'on-error', 'never']);
const usage = '`@decofe bench [blocks=N] [big-blocks[=true|false]] [bal=true|false|feature|baseline] [warmup=N] [baseline=REF] [feature=REF] [samply] [slack=always|on-win|on-error|never] [cores=N] [abba=true|false] [otlp=true|false] [wait-time=DURATION] [baseline-args="..."] [feature-args="..."]`';

async function checkOrgMembership({ github, context, core }) {
  const user = context.payload.comment.user.login;
  try {
    const { status } = await github.rest.orgs.checkMembershipForUser({
      org: 'paradigmxyz',
      username: user,
    });
    if (status !== 204 && status !== 302) {
      core.setFailed(`@${user} is not a member of paradigmxyz`);
    }
  } catch (e) {
    core.setFailed(`@${user} is not a member of paradigmxyz`);
  }
}

async function parseArguments({ github, context, core }) {
  let pr, actor, blocks, warmup, baseline, feature, samply, cores, bigBlocks, bal;
  let explicitWarmup = false;

  if (context.eventName === 'workflow_dispatch') {
    const inputs = context.payload.inputs || {};
    actor = process.env.GITHUB_ACTOR;
    blocks = inputs.blocks || '500';
    warmup = inputs.warmup || '200';
    if (warmup !== '200') explicitWarmup = true;
    baseline = inputs.baseline || '';
    feature = inputs.feature || '';
    samply = inputs.samply === 'true' ? 'true' : 'false';
    var slack = inputs.slack || 'never';
    cores = inputs.cores || '0';
    bigBlocks = inputs.big_blocks === 'true' ? 'true' : 'false';
    bal = inputs.bal || 'false';
    var abba = inputs.abba !== 'false' ? 'true' : 'false';
    var otlp = inputs.otlp !== 'false' ? 'true' : 'false';
    var waitTime = inputs.wait_time || '';
    var baselineNodeArgs = inputs.baseline_args || '';
    var featureNodeArgs = inputs.feature_args || '';

    const branch = process.env.GITHUB_REF_NAME;
    const { data: prs } = await github.rest.pulls.list({
      owner: context.repo.owner,
      repo: context.repo.repo,
      head: `${context.repo.owner}:${branch}`,
      state: 'open',
      per_page: 1,
    });
    pr = prs.length ? String(prs[0].number) : '';
    if (!pr) {
      core.info(`No open PR found for branch '${branch}', results will be in job summary`);
    }
  } else {
    pr = String(context.issue.number);
    actor = context.payload.comment.user.login;

    const body = context.payload.comment.body.trim();
    const intArgs = new Set(['warmup', 'cores', 'blocks']);
    const refArgs = new Set(['baseline', 'feature']);
    const boolArgs = new Set(['samply', 'big-blocks']);
    const boolDefaultTrue = new Set(['abba', 'otlp']);
    const enumArgs = new Map([['bal', validBalModes], ['slack', validSlackModes]]);
    const durationArgs = new Set(['wait-time']);
    const stringArgs = new Set(['baseline-args', 'feature-args']);
    const defaults = { blocks: '500', warmup: '200', baseline: '', feature: '', samply: 'false', slack: 'always', 'big-blocks': 'false', bal: 'false', cores: '0', abba: 'true', otlp: 'true', 'wait-time': '', 'baseline-args': '', 'feature-args': '' };
    const unknown = [];
    const invalid = [];
    const args = body.replace(/^(?:@decofe|derek) bench\s*/, '');
    const parts = [];
    const argRegex = /(\S+?="[^"]*"|\S+?='[^']*'|\S+)/g;
    let m;
    while ((m = argRegex.exec(args)) !== null) parts.push(m[1]);
    for (const part of parts) {
      const eq = part.indexOf('=');
      if (eq === -1) {
        if (boolArgs.has(part)) {
          defaults[part] = 'true';
        } else if (boolDefaultTrue.has(part)) {
          defaults[part] = 'true';
        } else {
          unknown.push(part);
        }
        continue;
      }
      const key = part.slice(0, eq);
      let value = part.slice(eq + 1);
      if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
        value = value.slice(1, -1);
      }
      if (boolArgs.has(key) || boolDefaultTrue.has(key)) {
        if (value === 'true' || value === 'false') {
          defaults[key] = value;
        } else {
          invalid.push(`\`${key}=${value}\` (must be true or false)`);
        }
      } else if (durationArgs.has(key)) {
        if (/^\d+(ms|s|m)$/.test(value)) {
          defaults[key] = value;
        } else {
          invalid.push(`\`${key}=${value}\` (must be a duration like 500ms, 1s, 2m)`);
        }
      } else if (enumArgs.has(key)) {
        if (enumArgs.get(key).has(value)) {
          defaults[key] = value;
        } else {
          invalid.push(`\`${key}=${value}\` (must be true, false, feature, or baseline)`);
        }
      } else if (intArgs.has(key)) {
        if (!/^\d+$/.test(value)) {
          invalid.push(`\`${key}=${value}\` (must be a positive integer)`);
        } else {
          defaults[key] = value;
          if (key === 'warmup') explicitWarmup = true;
        }
      } else if (refArgs.has(key)) {
        if (!value) {
          invalid.push(`\`${key}=\` (must be a git ref)`);
        } else {
          defaults[key] = value;
        }
      } else if (stringArgs.has(key)) {
        defaults[key] = value;
      } else {
        unknown.push(key);
      }
    }
    const errors = [];
    if (unknown.length) errors.push(`Unknown argument(s): \`${unknown.join('`, `')}\``);
    if (invalid.length) errors.push(`Invalid value(s): ${invalid.join(', ')}`);
    if (errors.length) {
      const msg = `❌ **Invalid bench command**\n\n${errors.join('\n')}\n\n**Usage:** ${usage}`;
      await github.rest.issues.createComment({
        owner: context.repo.owner,
        repo: context.repo.repo,
        issue_number: context.issue.number,
        body: msg,
      });
      core.setFailed(msg);
      return;
    }
    blocks = defaults.blocks;
    warmup = defaults.warmup;
    baseline = defaults.baseline;
    feature = defaults.feature;
    samply = defaults.samply;
    var slack = defaults.slack;
    cores = defaults.cores;
    bigBlocks = defaults['big-blocks'];
    bal = defaults.bal;
    var abba = defaults.abba;
    var otlp = defaults.otlp;
    var waitTime = defaults['wait-time'];
    var baselineNodeArgs = defaults['baseline-args'];
    var featureNodeArgs = defaults['feature-args'];
  }

  if (bigBlocks === 'true' && !explicitWarmup) {
    warmup = '20';
  }

  if (!validBalModes.has(bal)) {
    core.setFailed(`Invalid bal mode: ${bal}`);
    return;
  }
  if (bal !== 'false' && bigBlocks !== 'true') {
    const msg = `❌ **Invalid bench command**\n\n\`bal\` requires \`big-blocks=true\`.\n\n**Usage:** ${usage}`;
    if (context.eventName === 'issue_comment') {
      await github.rest.issues.createComment({
        owner: context.repo.owner,
        repo: context.repo.repo,
        issue_number: context.issue.number,
        body: msg,
      });
    }
    core.setFailed(msg);
    return;
  }

  let baselineName = baseline || 'main';
  let featureName = feature;
  if (!featureName) {
    if (pr) {
      const { data: prData } = await github.rest.pulls.get({
        owner: context.repo.owner,
        repo: context.repo.repo,
        pull_number: parseInt(pr),
      });
      featureName = prData.head.ref;
    } else {
      featureName = process.env.GITHUB_REF_NAME;
    }
  }

  core.setOutput('pr', pr || '');
  core.setOutput('actor', actor);
  core.setOutput('blocks', blocks);
  core.setOutput('warmup', warmup);
  core.setOutput('baseline', baseline);
  core.setOutput('feature', feature);
  core.setOutput('baseline-name', baselineName);
  core.setOutput('feature-name', featureName);
  core.setOutput('samply', samply);
  core.setOutput('slack', slack);
  core.setOutput('cores', cores);
  core.setOutput('big-blocks', bigBlocks);
  core.setOutput('bal', bal);
  core.setOutput('wait-time', waitTime);
  core.setOutput('baseline-args', baselineNodeArgs);
  core.setOutput('feature-args', featureNodeArgs);
  core.setOutput('abba', abba);
  core.setOutput('otlp', otlp);
}

function configLine({ blocks, warmup, baseline, feature, samply, slack, bigBlocks, bal, cores, abba, otlp, waitTime, baselineArgs, featureArgs, driver }) {
  const samplyNote = samply ? ', samply: `enabled`' : '';
  const slackNote = slack !== 'always' ? `, slack: \`${slack}\`` : '';
  const balNote = bigBlocks && bal !== 'false' ? `, BAL: \`${bal}\`` : '';
  const coresNote = cores && cores !== '0' ? `, cores: \`${cores}\`` : '';
  const abbaNote = !abba ? ', abba: `disabled`' : '';
  const otlpNote = !otlp ? ', otlp: `disabled`' : '';
  const waitTimeNote = waitTime ? `, wait-time: \`${waitTime}\`` : '';
  const baselineArgsNote = baselineArgs ? `, baseline-args: \`${baselineArgs}\`` : '';
  const featureArgsNote = featureArgs ? `, feature-args: \`${featureArgs}\`` : '';
  const driverNote = `, driver: \`${driver}\``;
  const blocksDesc = bigBlocks ? 'blocks: `big`' : `${blocks} blocks, ${warmup} warmup blocks`;
  return `**Config:** ${blocksDesc}, baseline: \`${baseline}\`, feature: \`${feature}\`${driverNote}${samplyNote}${slackNote}${balNote}${coresNote}${abbaNote}${otlpNote}${waitTimeNote}${baselineArgsNote}${featureArgsNote}`;
}

function configFromStepOutputs(outputs) {
  const bigBlocks = outputs['big-blocks'] === 'true';
  return configLine({
    blocks: outputs.blocks,
    warmup: outputs.warmup,
    baseline: outputs['baseline-name'],
    feature: outputs['feature-name'],
    samply: outputs.samply === 'true',
    slack: outputs.slack || 'always',
    bigBlocks,
    bal: outputs.bal || 'false',
    cores: outputs.cores,
    abba: outputs.abba !== 'false',
    otlp: outputs.otlp !== 'false',
    waitTime: outputs['wait-time'],
    baselineArgs: outputs['baseline-args'],
    featureArgs: outputs['feature-args'],
    driver: bigBlocks ? 'reth-bench' : 'txgen',
  });
}

async function getQueuePosition({ github, context }) {
  const numRunners = parseInt(process.env.BENCH_RUNNERS) || 1;
  const statuses = ['queued', 'in_progress', 'waiting', 'requested', 'pending'];
  const allRuns = [];
  for (const status of statuses) {
    const { data: { workflow_runs: r } } = await github.rest.actions.listWorkflowRuns({
      owner: context.repo.owner,
      repo: context.repo.repo,
      workflow_id: 'bench.yml',
      status,
      per_page: 100,
    });
    allRuns.push(...r);
  }
  const benchRuns = allRuns.filter(r => r.event === 'issue_comment' || r.event === 'workflow_dispatch');
  const thisRun = benchRuns.find(r => r.id === context.runId);
  const thisCreatedAt = thisRun ? new Date(thisRun.created_at) : new Date();
  const totalAhead = benchRuns.filter(r => r.id !== context.runId && new Date(r.created_at) <= thisCreatedAt).length;
  return { ahead: Math.max(0, totalAhead - numRunners + 1), numRunners };
}

async function acknowledgeRequest({ github, context, core, outputs }) {
  if (context.eventName === 'issue_comment') {
    await github.rest.reactions.createForIssueComment({
      owner: context.repo.owner,
      repo: context.repo.repo,
      comment_id: context.payload.comment.id,
      content: 'eyes',
    });
  }

  const pr = outputs.pr;
  if (!pr) return;

  const runUrl = `${context.serverUrl}/${context.repo.owner}/${context.repo.repo}/actions/runs/${context.runId}`;
  let queueMsg = '';
  let ahead = 0;
  try {
    const pos = await getQueuePosition({ github, context });
    ahead = pos.ahead;
    if (ahead > 0) {
      queueMsg = `\n🔢 **Queue position:** ${ahead} job(s) ahead (${pos.numRunners} runner(s))`;
    }
  } catch (e) {
    core.info(`Skipping queue tracking: ${e.message}`);
  }

  const actor = outputs.actor;
  const config = configFromStepOutputs(outputs);
  const { data: comment } = await github.rest.issues.createComment({
    owner: context.repo.owner,
    repo: context.repo.repo,
    issue_number: parseInt(pr),
    body: `cc @${actor}\n\n🚀 Benchmark queued! [View run](${runUrl})\n\n⏳ **Status:** Waiting for runner...${queueMsg}\n\n${config}`,
  });
  core.setOutput('comment-id', String(comment.id));
  core.setOutput('queue-position', String(ahead || 0));
}

async function pollQueuePosition({ github, context, core, outputs, commentId, initialPosition }) {
  const actor = outputs.actor;
  const config = configFromStepOutputs(outputs);
  const runUrl = `${context.serverUrl}/${context.repo.owner}/${context.repo.repo}/actions/runs/${context.runId}`;
  let lastPosition = parseInt(initialPosition);
  const sleep = ms => new Promise(r => setTimeout(r, ms));

  while (true) {
    await sleep(10_000);
    try {
      const { ahead, numRunners } = await getQueuePosition({ github, context });
      if (ahead !== lastPosition) {
        lastPosition = ahead;
        const queueMsg = ahead > 0
          ? `\n🔢 **Queue position:** ${ahead} job(s) ahead (${numRunners} runner(s))`
          : '';
        await github.rest.issues.updateComment({
          owner: context.repo.owner,
          repo: context.repo.repo,
          comment_id: parseInt(commentId),
          body: `cc @${actor}\n\n🚀 Benchmark queued! [View run](${runUrl})\n\n⏳ **Status:** Waiting for runner...${queueMsg}\n\n${config}`,
        });
      }
      if (ahead === 0) break;
    } catch (e) {
      core.info(`Queue poll error: ${e.message}`);
    }
  }
}

module.exports = {
  checkOrgMembership,
  parseArguments,
  acknowledgeRequest,
  pollQueuePosition,
  configLine,
};
