// Sends Slack notifications for benchmark results.
//
// Reads from environment:
//   SLACK_BENCH_BOT_TOKEN  – Slack Bot User OAuth Token (xoxb-...)
//   SLACK_BENCH_CHANNEL    – Public channel ID for significant improvements
//   BENCH_WORK_DIR         – Directory containing summary.json
//   BENCH_PR               – PR number (may be empty)
//   BENCH_ACTOR            – GitHub user who triggered the bench
//   BENCH_JOB_URL          – URL to the Actions job page
//   BENCH_BASELINE_ARGS    – Extra CLI args for the baseline reth node
//   BENCH_FEATURE_ARGS     – Extra CLI args for the feature reth node
//   BENCH_SAMPLY           – 'true' if samply profiling was enabled
//
// Usage from actions/github-script:
//   const notify = require('./.github/scripts/bench-slack-notify.js');
//   await notify.success({ core, context });
//   await notify.failure({ core, context, failedStep: '...' });

const fs = require('fs');
const path = require('path');
const { fmtChange, fmtMs, verdict, isWin, loadSamplyUrls, blocksLabel, metricRows } = require('./bench-utils');

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

async function postToSlack(token, channel, blocks, text, core, threadTs) {
  const payload = { channel, blocks, text, unfurl_links: false };
  if (threadTs) payload.thread_ts = threadTs;
  const resp = await fetch(SLACK_API, {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${token}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(payload),
  });
  const data = await resp.json();
  if (!data.ok) {
    core.warning(`Slack API error (channel ${channel}): ${JSON.stringify(data)}`);
  }
  return data;
}

function cell(text) {
  const s = String(text);
  return { type: 'raw_text', text: s || ' ' };
}

function profileLinks(samplyUrls, prefix) {
  return Object.entries(samplyUrls)
    .filter(([run]) => run.startsWith(`${prefix}-`))
    .sort(([a], [b]) => a.localeCompare(b, undefined, { numeric: true }))
    .map(([run, url]) => {
      const index = run.slice(prefix.length + 1);
      return `<${url}|Samply ${index}>`;
    });
}

// Slack shortcodes for verdict (Block Kit header doesn't support unicode emoji)
const SLACK_VERDICT = {
  '⚠️': ':warning:',
  '❌': ':x:',
  '✅': ':white_check_mark:',
  '⚪': ':white_circle:',
};

function benchConfigLine() {
  const parts = [];
  const add = (label, value, defaultValue = '') => {
    if (value && value !== defaultValue) {
      parts.push(`\`${label}=${value}\``);
    }
  };

  add('blocks', process.env.BENCH_BLOCKS);
  add('warmup', process.env.BENCH_WARMUP_BLOCKS);
  add('big-blocks', process.env.BENCH_BIG_BLOCKS, 'false');
  add('big-blocks-target-gas', process.env.BENCH_BIG_BLOCKS_TARGET_GAS);
  add('bal', process.env.BENCH_BAL, 'false');
  add('samply', process.env.BENCH_SAMPLY, 'false');
  add('slack', process.env.BENCH_SLACK, 'always');
  add('cores', process.env.BENCH_CORES, '0');
  add('run-pairs', process.env.BENCH_RUN_PAIRS);
  add('run-order', process.env.BENCH_RUN_ORDER);
  add('otlp', process.env.BENCH_OTLP, 'true');
  add('wait-time', process.env.BENCH_WAIT_TIME);
  add('baseline-args', process.env.BENCH_BASELINE_ARGS);
  add('feature-args', process.env.BENCH_FEATURE_ARGS);

  return parts.length ? `*Workflow:* ${parts.join(' ')}` : '';
}

function buildSuccessBlocks({ summary, prNumber, actor, actorSlackId, jobUrl, repo, samplyUrls }) {
  const { emoji, label } = verdict(summary.changes);
  const headerEmoji = SLACK_VERDICT[emoji] || emoji;

  const prUrl = prNumber ? `https://github.com/${repo}/pull/${prNumber}` : '';
  const commitUrl = `https://github.com/${repo}/commit`;
  const repoLink = `<https://github.com/${repo}|Reth>`;
  const baselineLink = `<${commitUrl}/${summary.baseline.ref}|${summary.baseline.name}>`;
  const featureLink = `<${commitUrl}/${summary.feature.ref}|${summary.feature.name}>`;

  // Meta line
  const metaParts = [`*Repo:* ${repoLink}`];
  if (prNumber) metaParts.push(`*<${prUrl}|PR #${prNumber}>*`);
  metaParts.push(`triggered by ${actorSlackId ? `<@${actorSlackId}>` : `@${actor}`}`);

  // Baseline/feature lines with samply profile links
  let baselineLine = `*Baseline:* ${baselineLink}`;
  const baselineProfiles = profileLinks(samplyUrls, 'baseline');
  if (baselineProfiles.length) baselineLine += ` | ${baselineProfiles.join(' | ')}`;

  let featureLine = `*Feature:* ${featureLink}`;
  const featureProfiles = profileLinks(samplyUrls, 'feature');
  if (featureProfiles.length) featureLine += ` | ${featureProfiles.join(' | ')}`;

  const countsLine = blocksLabel(summary).map(p => `*${p.key}:* ${p.value}`).join(' | ');
  const configLine = benchConfigLine();

  const baselineArgs = process.env.BENCH_BASELINE_ARGS || '';
  const featureArgs = process.env.BENCH_FEATURE_ARGS || '';
  const argsLines = [];
  if (baselineArgs) argsLines.push(`*Baseline Args:* \`${baselineArgs}\``);
  if (featureArgs) argsLines.push(`*Feature Args:* \`${featureArgs}\``);

  const sectionText = [metaParts.join(' | '), configLine, '', baselineLine, featureLine, ...argsLines, countsLine]
    .filter(line => line !== '')
    .join('\n');

  // Action buttons
  const diffUrl = `https://github.com/${repo}/compare/${summary.baseline.ref}...${summary.feature.ref}`;
  const buttons = [
    {
      type: 'button',
      text: { type: 'plain_text', text: 'CI :github:', emoji: true },
      url: jobUrl,
      action_id: 'ci_button',
    },
    {
      type: 'button',
      text: { type: 'plain_text', text: 'Diff :github:', emoji: true },
      url: diffUrl,
      action_id: 'diff_button',
    },
  ];

  // Build table rows from shared metricRows
  const rows = metricRows(summary);
  const tableRows = [
    [cell('Metric'), cell('Baseline'), cell('Feature'), cell('Change')],
    ...rows.map(r => [cell(r.label), cell(r.baseline), cell(r.feature), cell(r.change || ' ')]),
  ];

  const blocks = [
    {
      type: 'header',
      text: { type: 'plain_text', text: `${headerEmoji} ${label}`, emoji: true },
    },
    {
      type: 'section',
      text: { type: 'mrkdwn', text: sectionText },
    },
    {
      type: 'table',
      column_settings: [
        { align: 'left' },
        { align: 'right' },
        { align: 'right' },
        { align: 'right' },
      ],
      rows: tableRows,
    },
    {
      type: 'actions',
      elements: buttons,
    },
  ];

  return blocks;
}

function buildFailureBlocks({ prNumber, actor, actorSlackId, jobUrl, repo, failedStep }) {
  const prUrl = prNumber ? `https://github.com/${repo}/pull/${prNumber}` : '';
  const repoLink = `<https://github.com/${repo}|Reth>`;
  const actorMention = actorSlackId ? `<@${actorSlackId}>` : `@${actor}`;
  const parts = [
    `*Repo:* ${repoLink}`,
    prNumber ? `*<${prUrl}|PR #${prNumber}>*` : '',
    `by ${actorMention}`,
    `failed while *${failedStep}*`,
  ].filter(Boolean);
  const configLine = benchConfigLine();

  const buttons = [
    {
      type: 'button',
      text: { type: 'plain_text', text: 'CI :github:', emoji: true },
      url: jobUrl,
      action_id: 'ci_button',
    },
  ];

  return [
    {
      type: 'header',
      text: { type: 'plain_text', text: ':rotating_light: Bench Failed', emoji: true },
    },
    {
      type: 'section',
      text: { type: 'mrkdwn', text: [parts.join(' | '), configLine].filter(Boolean).join('\n') },
    },
    {
      type: 'actions',
      elements: buttons,
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

  const samplyUrls = loadSamplyUrls(process.env.BENCH_WORK_DIR);

  const slackUsers = loadSlackUsers(process.env.GITHUB_WORKSPACE || '.');
  const actorSlackId = slackUsers[actor];

  const blocks = buildSuccessBlocks({ summary, prNumber, actor, actorSlackId, jobUrl, repo, samplyUrls });
  const text = `Bench: ${summary.baseline.name} vs ${summary.feature.name}`;

  const slackMode = process.env.BENCH_SLACK || 'always';

  // Post to public channel only when the overall verdict is a win.
  const channel = process.env.SLACK_BENCH_CHANNEL;
  let postedToChannel = false;
  if (channel) {
    if (isWin(summary.changes)) {
      await postToSlack(token, channel, blocks, text, core);
      postedToChannel = true;
    } else {
      core.info('No unambiguous improvement, skipping public channel notification');
    }
  }

  // In on-win mode, only notify on unambiguous improvement. Mixed results are not wins.
  if (slackMode === 'on-win') {
    if (!postedToChannel) {
      core.info('on-win mode: no unambiguous improvement detected, skipping all notifications');
    }
    return;
  }

  // DM the actor only when results were not posted to the public channel
  if (!postedToChannel) {
    if (actorSlackId) {
      await postToSlack(token, actorSlackId, blocks, text, core);
    } else {
      core.info(`No Slack user mapping for GitHub user '${actor}', skipping DM`);
    }
  } else {
    core.info(`Results posted to channel, skipping DM to ${actor}`);
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

  const slackUsers = loadSlackUsers(process.env.GITHUB_WORKSPACE || '.');
  const actorSlackId = slackUsers[actor];

  const blocks = buildFailureBlocks({ prNumber, actor, actorSlackId, jobUrl, repo, failedStep });
  const text = `Bench failed while ${failedStep}`;

  // Always DM the actor
  if (actorSlackId) {
    await postToSlack(token, actorSlackId, blocks, text, core);
  } else {
    core.info(`No Slack user mapping for GitHub user '${actor}', skipping DM`);
  }

  // Only DM for failures, don't post to public channel
}

module.exports = { success, failure, benchConfigLine, buildSuccessBlocks, buildFailureBlocks };
