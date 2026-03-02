// Sends Slack notifications for reth-bench results.
//
// Reads from environment:
//   SLACK_BENCH_BOT_TOKEN  – Slack Bot User OAuth Token (xoxb-...)
//   SLACK_BENCH_CHANNEL    – Public channel ID for significant improvements
//   BENCH_WORK_DIR         – Directory containing summary.json
//   BENCH_PR               – PR number (may be empty)
//   BENCH_ACTOR            – GitHub user who triggered the bench
//   BENCH_JOB_URL          – URL to the Actions job page
//   BENCH_SAMPLY           – 'true' if samply profiling was enabled
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

function buildSuccessBlocks({ summary, prNumber, actor, actorSlackId, jobUrl, repo, samplyUrls }) {
  const b = summary.baseline.stats;
  const f = summary.feature.stats;
  const c = summary.changes;

  const sigEmoji = { good: '\u2705', bad: '\u274c', neutral: '\u26aa' };

  function fmtMs(v) { return v.toFixed(2) + 'ms'; }
  function fmtMgas(v) { return v.toFixed(2); }
  function fmtChange(ch) {
    if (!ch.pct && !ch.ci_pct) return ' ';
    const pctStr = `${ch.pct >= 0 ? '+' : ''}${ch.pct.toFixed(2)}%`;
    const ciStr = ch.ci_pct ? ` (\u00b1${ch.ci_pct.toFixed(2)}%)` : '';
    return `${pctStr}${ciStr} ${sigEmoji[ch.sig]}`;
  }

  // Overall result for header
  const vals = Object.values(c);
  const hasBad = vals.some(v => v.sig === 'bad');
  const hasGood = vals.some(v => v.sig === 'good');
  let headerEmoji, headerResult;
  if (hasBad && hasGood) {
    headerEmoji = ':warning:';
    headerResult = 'Mixed Results';
  } else if (hasBad) {
    headerEmoji = ':x:';
    headerResult = 'Regression';
  } else if (hasGood) {
    headerEmoji = ':white_check_mark:';
    headerResult = 'Improvement';
  } else {
    headerEmoji = ':white_circle:';
    headerResult = 'No Difference';
  }

  const prUrl = prNumber ? `https://github.com/${repo}/pull/${prNumber}` : '';
  const commitUrl = `https://github.com/${repo}/commit`;
  const baselineLink = `<${commitUrl}/${summary.baseline.ref}|${summary.baseline.name}>`;
  const featureLink = `<${commitUrl}/${summary.feature.ref}|${summary.feature.name}>`;

  // Meta line
  const metaParts = [];
  if (prNumber) metaParts.push(`*<${prUrl}|PR #${prNumber}>*`);
  metaParts.push(`triggered by ${actorSlackId ? `<@${actorSlackId}>` : `@${actor}`}`);

  // Baseline/feature lines with samply profile links
  let baselineLine = `*Baseline:* ${baselineLink}`;
  const bl1 = samplyUrls['baseline-1'];
  const bl2 = samplyUrls['baseline-2'];
  if (bl1) baselineLine += ` | <${bl1}|Samply 1>`;
  if (bl2) baselineLine += ` | <${bl2}|Samply 2>`;

  let featureLine = `*Feature:* ${featureLink}`;
  const fl1 = samplyUrls['feature-1'];
  const fl2 = samplyUrls['feature-2'];
  if (fl1) featureLine += ` | <${fl1}|Samply 1>`;
  if (fl2) featureLine += ` | <${fl2}|Samply 2>`;

  const warmup = summary.warmup_blocks || process.env.BENCH_WARMUP_BLOCKS || '';
  const cores = process.env.BENCH_CORES || '0';
  const countsParts = [];
  if (warmup) countsParts.push(`*Warmup:* ${warmup}`);
  countsParts.push(`*Blocks:* ${summary.blocks}`);
  if (cores !== '0') countsParts.push(`*Cores:* ${cores}`);
  const countsLine = countsParts.join(' | ');

  const sectionText = [metaParts.join(' | '), '', baselineLine, featureLine, countsLine].join('\n');

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

  const blocks = [
    {
      type: 'header',
      text: { type: 'plain_text', text: `${headerEmoji} ${headerResult}`, emoji: true },
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
      rows: [
        [cell('Metric'),  cell('Baseline'), cell('Feature'), cell('Change')],
        [cell('Mean'),     cell(fmtMs(b.mean_ms)),      cell(fmtMs(f.mean_ms)),      cell(fmtChange(c.mean))],
        [cell('StdDev'),   cell(fmtMs(b.stddev_ms)),    cell(fmtMs(f.stddev_ms)),    cell(' ')],
        [cell('P50'),      cell(fmtMs(b.p50_ms)),       cell(fmtMs(f.p50_ms)),       cell(fmtChange(c.p50))],
        [cell('P90'),      cell(fmtMs(b.p90_ms)),       cell(fmtMs(f.p90_ms)),       cell(fmtChange(c.p90))],
        [cell('P99'),      cell(fmtMs(b.p99_ms)),       cell(fmtMs(f.p99_ms)),       cell(fmtChange(c.p99))],
        [cell('Mgas/s'),   cell(fmtMgas(b.mean_mgas_s)), cell(fmtMgas(f.mean_mgas_s)), cell(fmtChange(c.mgas_s))],
      ],
    },
    {
      type: 'actions',
      elements: buttons,
    },
  ];

  // Wait times as a separate table block (sent as threaded reply due to Slack one-table limit)
  const threadBlocks = [];
  const waitTimes = summary.wait_times || {};
  const waitKeys = Object.keys(waitTimes);
  if (waitKeys.length > 0) {
    const waitRows = [
      [cell('Wait Time'), cell('Baseline'), cell('Feature')],
    ];
    for (const key of waitKeys) {
      const wt = waitTimes[key];
      waitRows.push([cell(wt.title), cell(fmtMs(wt.baseline.mean_ms)), cell(fmtMs(wt.feature.mean_ms))]);
    }
    threadBlocks.push({
      type: 'table',
      column_settings: [
        { align: 'left' },
        { align: 'right' },
        { align: 'right' },
      ],
      rows: waitRows,
    });
  }

  return { blocks, threadBlocks };
}

function buildFailureBlocks({ prNumber, actor, actorSlackId, jobUrl, repo, failedStep }) {
  const prUrl = prNumber ? `https://github.com/${repo}/pull/${prNumber}` : '';
  const actorMention = actorSlackId ? `<@${actorSlackId}>` : `@${actor}`;
  const parts = [
    prNumber ? `*<${prUrl}|PR #${prNumber}>*` : '',
    `by ${actorMention}`,
    `failed while *${failedStep}*`,
  ].filter(Boolean);

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
      text: { type: 'mrkdwn', text: parts.join(' | ') },
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

  // Load samply profile URLs (files exist when samply profiling was enabled)
  const samplyUrls = {};
  for (const run of ['baseline-1', 'baseline-2', 'feature-1', 'feature-2']) {
    try {
      const url = fs.readFileSync(
        path.join(process.env.BENCH_WORK_DIR, run, 'samply-profile-url.txt'), 'utf8'
      ).trim();
      if (url) samplyUrls[run] = url;
    } catch {}
  }

  const slackUsers = loadSlackUsers(process.env.GITHUB_WORKSPACE || '.');
  const actorSlackId = slackUsers[actor];

  const { blocks, threadBlocks } = buildSuccessBlocks({ summary, prNumber, actor, actorSlackId, jobUrl, repo, samplyUrls });
  const text = `Bench: ${summary.baseline.name} vs ${summary.feature.name}`;

  async function sendWithThread(ch) {
    const res = await postToSlack(token, ch, blocks, text, core);
    if (res.ok && res.ts && threadBlocks.length > 0) {
      for (const tb of threadBlocks) {
        await postToSlack(token, ch, [tb], 'Wait time breakdown', core, res.ts);
      }
    }
  }

  // Post to public channel if any metric shows significant improvement or regression
  const channel = process.env.SLACK_BENCH_CHANNEL;
  let postedToChannel = false;
  if (channel) {
    const changes = summary.changes || {};
    const hasImprovement = Object.values(changes).some(c => c.sig === 'good');
    if (hasImprovement) {
      await sendWithThread(channel);
      postedToChannel = true;
    } else {
      core.info('No significant improvement, skipping public channel notification');
    }
  }

  // DM the actor only when results were not posted to the public channel
  if (!postedToChannel) {
    if (actorSlackId) {
      await sendWithThread(actorSlackId);
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

module.exports = { success, failure };
