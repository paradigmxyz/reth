// Sends Slack notifications for reth-bench results.
//
// Reads from environment:
//   SLACK_BENCH_BOT_TOKEN  – Slack Bot User OAuth Token (xoxb-...)
//   SLACK_BENCH_CHANNEL    – Public channel ID for significant improvements
//   BENCH_WORK_DIR         – Directory containing summary.json
//   BENCH_PR               – PR number (may be empty)
//   BENCH_ACTOR            – GitHub user who triggered the bench
//   BENCH_JOB_URL          – URL to the Actions job page
//
// Usage from actions/github-script:
//   const notify = require('./.github/scripts/bench-slack-notify.js');
//   await notify.success({ core, context });
//   await notify.failure({ core, context, failedStep: '...' });

const fs = require('fs');
const path = require('path');

const SLACK_API = 'https://slack.com/api/chat.postMessage';

function loadSlackUsers(repoRoot) {
  try {
    const raw = fs.readFileSync(path.join(repoRoot, '.github', 'scripts', 'bench-slack-users.json'), 'utf8');
    const data = JSON.parse(raw);
    // Filter out non-user-ID entries (like _comment)
    const users = {};
    for (const [k, v] of Object.entries(data)) {
      if (!k.startsWith('_') && typeof v === 'string' && v.startsWith('U')) {
        users[k] = v;
      }
    }
    return users;
  } catch {
    return {};
  }
}

async function postToSlack(token, channel, blocks, core) {
  const resp = await fetch(SLACK_API, {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${token}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ channel, blocks, unfurl_links: false }),
  });
  const data = await resp.json();
  if (!data.ok) {
    core.warning(`Slack API error (channel ${channel}): ${data.error}`);
  }
  return data;
}

function buildSuccessBlocks({ summary, prNumber, actor, jobUrl, repo }) {
  const b = summary.baseline.stats;
  const f = summary.feature.stats;
  const c = summary.changes;

  const sigEmoji = { good: ':white_check_mark:', bad: ':x:', neutral: ':white_circle:' };

  function fmtChange(ch) {
    const e = sigEmoji[ch.sig];
    return `${ch.pct >= 0 ? '+' : ''}${ch.pct.toFixed(2)}% ${e}`;
  }

  const prUrl = prNumber ? `https://github.com/${repo}/pull/${prNumber}` : '';
  const meta = prNumber
    ? `*<${prUrl}|PR #${prNumber}>* by @${actor} | <${jobUrl}|View job> | ${summary.blocks} blocks`
    : `Triggered by @${actor} | <${jobUrl}|View job> | ${summary.blocks} blocks`;

  const blocks = [
    {
      type: 'header',
      text: { type: 'plain_text', text: `Bench: ${summary.baseline.name} vs ${summary.feature.name}`, emoji: true },
    },
    {
      type: 'section',
      text: { type: 'mrkdwn', text: meta },
    },
    { type: 'divider' },
    {
      type: 'section',
      text: { type: 'mrkdwn', text: '*Latency (newPayload)*' },
      fields: [
        { type: 'mrkdwn', text: '*Metric*' },
        { type: 'mrkdwn', text: '*Change*' },
        { type: 'mrkdwn', text: `Mean: ${b.mean_ms.toFixed(2)} \u2192 ${f.mean_ms.toFixed(2)} ms` },
        { type: 'mrkdwn', text: fmtChange(c.mean) },
        { type: 'mrkdwn', text: `P50: ${b.p50_ms.toFixed(2)} \u2192 ${f.p50_ms.toFixed(2)} ms` },
        { type: 'mrkdwn', text: fmtChange(c.p50) },
        { type: 'mrkdwn', text: `P90: ${b.p90_ms.toFixed(2)} \u2192 ${f.p90_ms.toFixed(2)} ms` },
        { type: 'mrkdwn', text: fmtChange(c.p90) },
        { type: 'mrkdwn', text: `P99: ${b.p99_ms.toFixed(2)} \u2192 ${f.p99_ms.toFixed(2)} ms` },
        { type: 'mrkdwn', text: fmtChange(c.p99) },
      ],
    },
    { type: 'divider' },
    {
      type: 'section',
      fields: [
        { type: 'mrkdwn', text: '*Throughput*' },
        { type: 'mrkdwn', text: '*Change*' },
        { type: 'mrkdwn', text: `Mgas/s: ${b.mean_mgas_s.toFixed(2)} \u2192 ${f.mean_mgas_s.toFixed(2)}` },
        { type: 'mrkdwn', text: fmtChange(c.mgas_s) },
      ],
    },
  ];

  // Wait times
  const waitTimes = summary.wait_times || {};
  const waitKeys = Object.keys(waitTimes);
  if (waitKeys.length > 0) {
    const waitFields = [
      { type: 'mrkdwn', text: '*Wait Time*' },
      { type: 'mrkdwn', text: '*Base \u2192 Feature (mean)*' },
    ];
    for (const key of waitKeys) {
      const wt = waitTimes[key];
      waitFields.push({ type: 'mrkdwn', text: wt.title });
      waitFields.push({
        type: 'mrkdwn',
        text: `${wt.baseline.mean_ms.toFixed(2)} \u2192 ${wt.feature.mean_ms.toFixed(2)} ms`,
      });
    }
    blocks.push({ type: 'divider' });
    blocks.push({ type: 'section', fields: waitFields });
  }

  // Footer
  blocks.push({
    type: 'context',
    elements: [{
      type: 'mrkdwn',
      text: `${repo} | ${summary.baseline.name} (\`${summary.baseline.ref.slice(0, 8)}\`) vs ${summary.feature.name} (\`${summary.feature.ref.slice(0, 8)}\`)`,
    }],
  });

  return blocks;
}

function buildFailureBlocks({ prNumber, actor, jobUrl, repo, failedStep }) {
  const prUrl = prNumber ? `https://github.com/${repo}/pull/${prNumber}` : '';
  const parts = [
    prNumber ? `*<${prUrl}|PR #${prNumber}>*` : '',
    `Triggered by @${actor}`,
    `Failed while *${failedStep}*`,
    `<${jobUrl}|View logs>`,
  ].filter(Boolean);

  return [
    {
      type: 'header',
      text: { type: 'plain_text', text: ':rotating_light: Bench Failed', emoji: true },
    },
    {
      type: 'section',
      text: { type: 'mrkdwn', text: parts.join(' | ') },
    },
  ];
}

async function success({ core, context }) {
  const token = process.env.SLACK_BENCH_BOT_TOKEN;
  if (!token) {
    core.info('SLACK_BENCH_BOT_TOKEN not set, skipping Slack notification');
    return;
  }

  let summary;
  try {
    summary = JSON.parse(fs.readFileSync(process.env.BENCH_WORK_DIR + '/summary.json', 'utf8'));
  } catch (e) {
    core.warning('Could not read summary.json for Slack notification');
    return;
  }

  const repo = `${context.repo.owner}/${context.repo.repo}`;
  const prNumber = process.env.BENCH_PR;
  const actor = process.env.BENCH_ACTOR;
  const jobUrl = process.env.BENCH_JOB_URL ||
    `${context.serverUrl}/${context.repo.owner}/${context.repo.repo}/actions/runs/${context.runId}`;

  const blocks = buildSuccessBlocks({ summary, prNumber, actor, jobUrl, repo });
  const slackUsers = loadSlackUsers(process.env.GITHUB_WORKSPACE || '.');

  // Always DM the actor
  const actorSlackId = slackUsers[actor];
  if (actorSlackId) {
    await postToSlack(token, actorSlackId, blocks, core);
  } else {
    core.info(`No Slack user mapping for GitHub user '${actor}', skipping DM`);
  }

  // Post to public channel if any metric shows significant improvement
  const channel = process.env.SLACK_BENCH_CHANNEL;
  if (channel) {
    const changes = summary.changes || {};
    const hasImprovement = Object.values(changes).some(c => c.sig === 'good');
    if (hasImprovement) {
      await postToSlack(token, channel, blocks, core);
    } else {
      core.info('No significant improvement, skipping public channel notification');
    }
  }
}

async function failure({ core, context, failedStep }) {
  const token = process.env.SLACK_BENCH_BOT_TOKEN;
  if (!token) {
    core.info('SLACK_BENCH_BOT_TOKEN not set, skipping Slack notification');
    return;
  }

  const repo = `${context.repo.owner}/${context.repo.repo}`;
  const prNumber = process.env.BENCH_PR;
  const actor = process.env.BENCH_ACTOR;
  const jobUrl = process.env.BENCH_JOB_URL ||
    `${context.serverUrl}/${context.repo.owner}/${context.repo.repo}/actions/runs/${context.runId}`;

  const blocks = buildFailureBlocks({ prNumber, actor, jobUrl, repo, failedStep });
  const slackUsers = loadSlackUsers(process.env.GITHUB_WORKSPACE || '.');

  // Always DM the actor
  const actorSlackId = slackUsers[actor];
  if (actorSlackId) {
    await postToSlack(token, actorSlackId, blocks, core);
  } else {
    core.info(`No Slack user mapping for GitHub user '${actor}', skipping DM`);
  }

  // Always post failures to public channel
  const channel = process.env.SLACK_BENCH_CHANNEL;
  if (channel) {
    await postToSlack(token, channel, blocks, core);
  }
}

module.exports = { success, failure };
